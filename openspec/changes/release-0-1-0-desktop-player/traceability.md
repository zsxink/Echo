# 0.1.0 场景级需求追踪

## 完成规则

- 本表逐项覆盖 specs 中的全部 Scenario；稳定 ID 不得复用，标题调整时保留原 ID。
- 任务 1.3 建立测试 manifest 与执行器。每行指定的 manifest 必须包含正常路径、关键失败路径、fixture、预期结果和自动测试过滤器；Native 行还必须包含平台版本、人工步骤、操作者和证据路径。
- 每行给出实际验收命令。`pnpm verify:scenario -- --all` 必须比较 specs、此表和测试 manifest 的 ID 集合完全相等，并逐项执行；缺失、重复、无命令、无证据或失败均返回非零。
- 当前基线：55 个 Requirement，160 个 Scenario。Requirement 计数只用于审计，不再用整行继承代替场景映射。

## desktop-app-shell

| Scenario ID | Requirement | Scenario | 任务 | 测试层 | 测试/步骤 manifest | 实际验收命令 |
|---|---|---|---|---|---|---|
| DAS-R01-S01 | 首次启动必须建立本机工作区 | 首次选择有效目录 | 4.1, 7.1, 10.3, 13.2 | Core/React/E2E | `tests/scenarios/DAS-R01-S01.yaml` | `pnpm verify:scenario -- DAS-R01-S01` |
| DAS-R01-S02 | 首次启动必须建立本机工作区 | 取消选择目录 | 4.1, 7.1, 10.3, 13.2 | Core/React/E2E | `tests/scenarios/DAS-R01-S02.yaml` | `pnpm verify:scenario -- DAS-R01-S02` |
| DAS-R01-S03 | 首次启动必须建立本机工作区 | 选择不可用目录 | 4.1, 7.1, 10.3, 13.2 | Core/React/E2E | `tests/scenarios/DAS-R01-S03.yaml` | `pnpm verify:scenario -- DAS-R01-S03` |
| DAS-R01-S04 | 首次启动必须建立本机工作区 | 已有本机配置再次启动 | 4.1, 7.1, 10.3, 13.2 | Core/React/E2E | `tests/scenarios/DAS-R01-S04.yaml` | `pnpm verify:scenario -- DAS-R01-S04` |
| DAS-R02-S01 | 应用框架必须提供稳定的目录入口 | 从侧边导航切换资料库视图 | 10.4, 12.3 | React/E2E | `tests/scenarios/DAS-R02-S01.yaml` | `pnpm verify:scenario -- DAS-R02-S01` |
| DAS-R02-S02 | 应用框架必须提供稳定的目录入口 | 视图或资料库为空 | 10.4, 12.3 | React/E2E | `tests/scenarios/DAS-R02-S02.yaml` | `pnpm verify:scenario -- DAS-R02-S02` |
| DAS-R02-S03 | 应用框架必须提供稳定的目录入口 | 访问一期明确排除的入口 | 10.4, 12.3 | React/E2E | `tests/scenarios/DAS-R02-S03.yaml` | `pnpm verify:scenario -- DAS-R02-S03` |
| DAS-R03-S01 | 主题与本机偏好必须持久化且可回退 | 切换主题 | 7.6, 10.1 | Desktop/React | `tests/scenarios/DAS-R03-S01.yaml` | `pnpm verify:scenario -- DAS-R03-S01` |
| DAS-R03-S02 | 主题与本机偏好必须持久化且可回退 | 下次启动恢复主题 | 7.6, 10.1 | Desktop/React | `tests/scenarios/DAS-R03-S02.yaml` | `pnpm verify:scenario -- DAS-R03-S02` |
| DAS-R03-S03 | 主题与本机偏好必须持久化且可回退 | 偏好不可保存或值非法 | 7.6, 10.1 | Desktop/React | `tests/scenarios/DAS-R03-S03.yaml` | `pnpm verify:scenario -- DAS-R03-S03` |
| DAS-R04-S01 | 主窗口关闭行为必须符合用户选择并保留播放状态 | 选择退出应用 | 7.6, 9.6, 13.5 | Desktop/Native | `tests/native/DAS-R04-S01.md` | `pnpm verify:scenario -- DAS-R04-S01` |
| DAS-R04-S02 | 主窗口关闭行为必须符合用户选择并保留播放状态 | 选择后台运行 | 7.6, 9.6, 13.5 | Desktop/Native | `tests/native/DAS-R04-S02.md` | `pnpm verify:scenario -- DAS-R04-S02` |
| DAS-R04-S03 | 主窗口关闭行为必须符合用户选择并保留播放状态 | 关闭期间存在未完成初始化 | 7.6, 9.6, 13.5 | Desktop/Native | `tests/native/DAS-R04-S03.md` | `pnpm verify:scenario -- DAS-R04-S03` |
| DAS-R05-S01 | 系统托盘与菜单栏入口必须提供一致的后台控制 | 从平台入口显示主窗口 | 9.3, 13.5 | Desktop/Native | `tests/native/DAS-R05-S01.md` | `pnpm verify:scenario -- DAS-R05-S01` |
| DAS-R05-S02 | 系统托盘与菜单栏入口必须提供一致的后台控制 | 从平台入口控制播放 | 9.3, 13.5 | Desktop/Native | `tests/native/DAS-R05-S02.md` | `pnpm verify:scenario -- DAS-R05-S02` |
| DAS-R05-S03 | 系统托盘与菜单栏入口必须提供一致的后台控制 | 托盘或菜单栏初始化失败 | 9.3, 13.5 | Desktop/Native | `tests/native/DAS-R05-S03.md` | `pnpm verify:scenario -- DAS-R05-S03` |
| DAS-R06-S01 | 应用必须保证单实例与文件打开唤醒 | 重复启动应用 | 7.1, 9.1, 9.2 | Desktop/Native | `tests/native/DAS-R06-S01.md` | `pnpm verify:scenario -- DAS-R06-S01` |
| DAS-R06-S02 | 应用必须保证单实例与文件打开唤醒 | 系统文件关联触发打开 | 7.1, 9.1, 9.2 | Desktop/Native | `tests/native/DAS-R06-S02.md` | `pnpm verify:scenario -- DAS-R06-S02` |
| DAS-R06-S03 | 应用必须保证单实例与文件打开唤醒 | 首实例尚未就绪 | 7.1, 9.1, 9.2 | Desktop/Native | `tests/native/DAS-R06-S03.md` | `pnpm verify:scenario -- DAS-R06-S03` |
| DAS-R07-S01 | 窄屏布局与浮层关闭必须可预测 | 打开和关闭窄屏侧边栏 | 10.4, 12.1, 12.3 | React/E2E | `tests/scenarios/DAS-R07-S01.yaml` | `pnpm verify:scenario -- DAS-R07-S01` |
| DAS-R07-S02 | 窄屏布局与浮层关闭必须可预测 | 窗口从宽屏变为窄屏 | 10.4, 12.1, 12.3 | React/E2E | `tests/scenarios/DAS-R07-S02.yaml` | `pnpm verify:scenario -- DAS-R07-S02` |
| DAS-R07-S03 | 窄屏布局与浮层关闭必须可预测 | 浮层点击外部 | 10.4, 12.1, 12.3 | React/E2E | `tests/scenarios/DAS-R07-S03.yaml` | `pnpm verify:scenario -- DAS-R07-S03` |
| DAS-R08-S01 | 键盘焦点与 Escape 行为必须可访问 | 键盘遍历应用壳 | 11.8, 12.1, 12.2 | React/E2E | `tests/scenarios/DAS-R08-S01.yaml` | `pnpm verify:scenario -- DAS-R08-S01` |
| DAS-R08-S02 | 键盘焦点与 Escape 行为必须可访问 | Escape 关闭最上层浮层 | 11.8, 12.1, 12.2 | React/E2E | `tests/scenarios/DAS-R08-S02.yaml` | `pnpm verify:scenario -- DAS-R08-S02` |
| DAS-R08-S03 | 键盘焦点与 Escape 行为必须可访问 | 使用辅助技术 | 11.8, 12.1, 12.2 | React/E2E | `tests/scenarios/DAS-R08-S03.yaml` | `pnpm verify:scenario -- DAS-R08-S03` |
| DAS-R09-S01 | 三平台壳行为必须可验证且不依赖网络 | 离线启动 | 1.5, 7.7, 9.8, 13.5, 13.7, 13.8 | Security/Native | `tests/native/DAS-R09-S01.md` | `pnpm verify:scenario -- DAS-R09-S01` |
| DAS-R09-S02 | 三平台壳行为必须可验证且不依赖网络 | 跨平台验证 | 1.5, 7.7, 9.8, 13.5, 13.7, 13.8 | Security/Native | `tests/native/DAS-R09-S02.md` | `pnpm verify:scenario -- DAS-R09-S02` |

## desktop-playback

| Scenario ID | Requirement | Scenario | 任务 | 测试层 | 测试/步骤 manifest | 实际验收命令 |
|---|---|---|---|---|---|---|
| DP-R01-S01 | 桌面音频播放 | 播放资料库歌曲 | 8.1–8.4, 8.12 | Desktop/Native | `tests/native/DP-R01-S01.md` | `pnpm verify:scenario -- DP-R01-S01` |
| DP-R01-S02 | 桌面音频播放 | 文件不可用 | 8.1–8.4, 8.12 | Desktop/Native | `tests/native/DP-R01-S02.md` | `pnpm verify:scenario -- DP-R01-S02` |
| DP-R02-S01 | 播放队列 | 从曲库开始播放 | 8.5, 8.6, 11.2 | Desktop/React | `tests/scenarios/DP-R02-S01.yaml` | `pnpm verify:scenario -- DP-R02-S01` |
| DP-R02-S02 | 播放队列 | 加入队列 | 8.5, 8.6, 11.2 | Desktop/React | `tests/scenarios/DP-R02-S02.yaml` | `pnpm verify:scenario -- DP-R02-S02` |
| DP-R02-S03 | 播放队列 | 下一首播放 | 8.5, 8.6, 11.2 | Desktop/React | `tests/scenarios/DP-R02-S03.yaml` | `pnpm verify:scenario -- DP-R02-S03` |
| DP-R02-S04 | 播放队列 | 清空队列 | 8.5, 8.6, 11.2 | Desktop/React | `tests/scenarios/DP-R02-S04.yaml` | `pnpm verify:scenario -- DP-R02-S04` |
| DP-R03-S01 | 播放模式与传输控制 | 基础传输控制 | 8.6, 8.8, 11.1 | Desktop/React | `tests/scenarios/DP-R03-S01.yaml` | `pnpm verify:scenario -- DP-R03-S01` |
| DP-R03-S02 | 播放模式与传输控制 | 切换播放模式 | 8.6, 8.8, 11.1 | Desktop/React | `tests/scenarios/DP-R03-S02.yaml` | `pnpm verify:scenario -- DP-R03-S02` |
| DP-R03-S03 | 播放模式与传输控制 | 定位和音量 | 8.6, 8.8, 11.1 | Desktop/React | `tests/scenarios/DP-R03-S03.yaml` | `pnpm verify:scenario -- DP-R03-S03` |
| DP-R04-S01 | 播放错误处理 | 单曲加载失败 | 8.7, 11.2 | Desktop/React | `tests/scenarios/DP-R04-S01.yaml` | `pnpm verify:scenario -- DP-R04-S01` |
| DP-R04-S02 | 播放错误处理 | 单曲循环中的加载失败 | 8.7, 11.2 | Desktop/React | `tests/scenarios/DP-R04-S02.yaml` | `pnpm verify:scenario -- DP-R04-S02` |
| DP-R04-S03 | 播放错误处理 | 资料库不可用期间播放 | 8.7, 11.2 | Desktop/React | `tests/scenarios/DP-R04-S03.yaml` | `pnpm verify:scenario -- DP-R04-S03` |
| DP-R05-S01 | 临时播放项边界 | 打开资料库外文件 | 9.2, 11.7 | Desktop/Native | `tests/native/DP-R05-S01.md` | `pnpm verify:scenario -- DP-R05-S01` |
| DP-R05-S02 | 临时播放项边界 | 临时项执行持久化操作 | 9.2, 11.7 | Desktop/Native | `tests/native/DP-R05-S02.md` | `pnpm verify:scenario -- DP-R05-S02` |
| DP-R06-S01 | 播放统计 | 达到播放阈值 | 3.9, 8.10 | Core/Desktop | `tests/scenarios/DP-R06-S01.yaml` | `pnpm verify:scenario -- DP-R06-S01` |
| DP-R06-S02 | 播放统计 | 未达到阈值或 seek 作弊 | 3.9, 8.10 | Core/Desktop | `tests/scenarios/DP-R06-S02.yaml` | `pnpm verify:scenario -- DP-R06-S02` |
| DP-R07-S01 | 会话恢复 | 正常恢复 | 7.6, 8.9 | Desktop | `tests/scenarios/DP-R07-S01.yaml` | `pnpm verify:scenario -- DP-R07-S01` |
| DP-R07-S02 | 会话恢复 | 部分歌曲不可用 | 7.6, 8.9 | Desktop | `tests/scenarios/DP-R07-S02.yaml` | `pnpm verify:scenario -- DP-R07-S02` |
| DP-R07-S03 | 会话恢复 | 文件关联覆盖普通恢复 | 7.6, 8.9 | Desktop | `tests/scenarios/DP-R07-S03.yaml` | `pnpm verify:scenario -- DP-R07-S03` |
| DP-R08-S01 | 媒体键与快捷键 | 使用媒体键 | 9.4, 11.8 | Desktop/React/Native | `tests/native/DP-R08-S01.md` | `pnpm verify:scenario -- DP-R08-S01` |
| DP-R08-S02 | 媒体键与快捷键 | 使用应用快捷键 | 9.4, 11.8 | Desktop/React/Native | `tests/native/DP-R08-S02.md` | `pnpm verify:scenario -- DP-R08-S02` |
| DP-R09-S01 | 后台与托盘控制 | 关闭窗口退出 | 9.3, 9.6 | Desktop/Native | `tests/native/DP-R09-S01.md` | `pnpm verify:scenario -- DP-R09-S01` |
| DP-R09-S02 | 后台与托盘控制 | 关闭窗口后台运行 | 9.3, 9.6 | Desktop/Native | `tests/native/DP-R09-S02.md` | `pnpm verify:scenario -- DP-R09-S02` |
| DP-R09-S03 | 后台与托盘控制 | 托盘控制 | 9.3, 9.6 | Desktop/Native | `tests/native/DP-R09-S03.md` | `pnpm verify:scenario -- DP-R09-S03` |

## immersive-lyrics

| Scenario ID | Requirement | Scenario | 任务 | 测试层 | 测试/步骤 manifest | 实际验收命令 |
|---|---|---|---|---|---|---|
| IL-R01-S01 | 常驻播放栏必须始终提供当前播放与基础控制 | 开始播放歌曲 | 10.4, 11.1 | React/E2E | `tests/scenarios/IL-R01-S01.yaml` | `pnpm verify:scenario -- IL-R01-S01` |
| IL-R01-S02 | 常驻播放栏必须始终提供当前播放与基础控制 | 播放项没有封面 | 10.4, 11.1 | React/E2E | `tests/scenarios/IL-R01-S02.yaml` | `pnpm verify:scenario -- IL-R01-S02` |
| IL-R01-S03 | 常驻播放栏必须始终提供当前播放与基础控制 | 当前播放结束 | 10.4, 11.1 | React/E2E | `tests/scenarios/IL-R01-S03.yaml` | `pnpm verify:scenario -- IL-R01-S03` |
| IL-R02-S01 | 播放栏的传输控制与播放模式必须可预测 | 切换播放模式 | 8.6, 8.8, 11.1 | Desktop/React | `tests/scenarios/IL-R02-S01.yaml` | `pnpm verify:scenario -- IL-R02-S01` |
| IL-R02-S02 | 播放栏的传输控制与播放模式必须可预测 | 静音后恢复 | 8.6, 8.8, 11.1 | Desktop/React | `tests/scenarios/IL-R02-S02.yaml` | `pnpm verify:scenario -- IL-R02-S02` |
| IL-R02-S03 | 播放栏的传输控制与播放模式必须可预测 | 播放器命令失败 | 8.6, 8.8, 11.1 | Desktop/React | `tests/scenarios/IL-R02-S03.yaml` | `pnpm verify:scenario -- IL-R02-S03` |
| IL-R03-S01 | 沉浸式播放器必须展示封面、元信息并保持基础控制 | 展开沉浸式播放器 | 11.3 | React/E2E | `tests/scenarios/IL-R03-S01.yaml` | `pnpm verify:scenario -- IL-R03-S01` |
| IL-R03-S02 | 沉浸式播放器必须展示封面、元信息并保持基础控制 | 播放项切换期间保持沉浸模式 | 11.3 | React/E2E | `tests/scenarios/IL-R03-S02.yaml` | `pnpm verify:scenario -- IL-R03-S02` |
| IL-R03-S03 | 沉浸式播放器必须展示封面、元信息并保持基础控制 | 窄屏沉浸模式 | 11.3 | React/E2E | `tests/scenarios/IL-R03-S03.yaml` | `pnpm verify:scenario -- IL-R03-S03` |
| IL-R04-S01 | 歌词来源优先级必须可见且一致 | 多个来源同时存在 | 4.5, 11.5 | Core/React | `tests/scenarios/IL-R04-S01.yaml` | `pnpm verify:scenario -- IL-R04-S01` |
| IL-R04-S02 | 歌词来源优先级必须可见且一致 | 仅有内嵌歌词 | 4.5, 11.5 | Core/React | `tests/scenarios/IL-R04-S02.yaml` | `pnpm verify:scenario -- IL-R04-S02` |
| IL-R04-S03 | 歌词来源优先级必须可见且一致 | 仅有同名 LRC | 4.5, 11.5 | Core/React | `tests/scenarios/IL-R04-S03.yaml` | `pnpm verify:scenario -- IL-R04-S03` |
| IL-R04-S04 | 歌词来源优先级必须可见且一致 | 歌词来源读取失败 | 4.5, 11.5 | Core/React | `tests/scenarios/IL-R04-S04.yaml` | `pnpm verify:scenario -- IL-R04-S04` |
| IL-R05-S01 | 带时间戳歌词必须随播放进度同步 | 正常同步与换行 | 4.5, 11.4 | Core/React | `tests/scenarios/IL-R05-S01.yaml` | `pnpm verify:scenario -- IL-R05-S01` |
| IL-R05-S02 | 带时间戳歌词必须随播放进度同步 | 点击歌词 seek | 4.5, 11.4 | Core/React | `tests/scenarios/IL-R05-S02.yaml` | `pnpm verify:scenario -- IL-R05-S02` |
| IL-R05-S03 | 带时间戳歌词必须随播放进度同步 | 时间戳超出歌曲范围或顺序异常 | 4.5, 11.4 | Core/React | `tests/scenarios/IL-R05-S03.yaml` | `pnpm verify:scenario -- IL-R05-S03` |
| IL-R06-S01 | 无时间戳歌词与无歌词状态必须明确 | 仅有纯文本歌词 | 4.5, 11.5 | Core/React | `tests/scenarios/IL-R06-S01.yaml` | `pnpm verify:scenario -- IL-R06-S01` |
| IL-R06-S02 | 无时间戳歌词与无歌词状态必须明确 | 没有歌词 | 4.5, 11.5 | Core/React | `tests/scenarios/IL-R06-S02.yaml` | `pnpm verify:scenario -- IL-R06-S02` |
| IL-R06-S03 | 无时间戳歌词与无歌词状态必须明确 | 切换到无歌词歌曲 | 4.5, 11.5 | Core/React | `tests/scenarios/IL-R06-S03.yaml` | `pnpm verify:scenario -- IL-R06-S03` |
| IL-R07-S01 | 歌词专注模式必须支持进入、阅读与退出 | 进入歌词专注阅读 | 11.6 | React/E2E | `tests/scenarios/IL-R07-S01.yaml` | `pnpm verify:scenario -- IL-R07-S01` |
| IL-R07-S02 | 歌词专注模式必须支持进入、阅读与退出 | 专注模式中控制播放 | 11.6 | React/E2E | `tests/scenarios/IL-R07-S02.yaml` | `pnpm verify:scenario -- IL-R07-S02` |
| IL-R07-S03 | 歌词专注模式必须支持进入、阅读与退出 | 退出专注模式 | 11.6 | React/E2E | `tests/scenarios/IL-R07-S03.yaml` | `pnpm verify:scenario -- IL-R07-S03` |
| IL-R08-S01 | 歌词滚动、浮层与焦点不得互相遮挡 | 手动滚动歌词 | 11.6, 12.1 | React/E2E | `tests/scenarios/IL-R08-S01.yaml` | `pnpm verify:scenario -- IL-R08-S01` |
| IL-R08-S02 | 歌词滚动、浮层与焦点不得互相遮挡 | 恢复跟随当前行 | 11.6, 12.1 | React/E2E | `tests/scenarios/IL-R08-S02.yaml` | `pnpm verify:scenario -- IL-R08-S02` |
| IL-R08-S03 | 歌词滚动、浮层与焦点不得互相遮挡 | 播放队列浮层覆盖 | 11.6, 12.1 | React/E2E | `tests/scenarios/IL-R08-S03.yaml` | `pnpm verify:scenario -- IL-R08-S03` |
| IL-R09-S01 | 沉浸式歌词体验必须适配减少动画与三平台 | 减少动画偏好 | 11.3, 12.3, 12.4 | React | `tests/scenarios/IL-R09-S01.yaml` | `pnpm verify:scenario -- IL-R09-S01` |
| IL-R09-S02 | 沉浸式歌词体验必须适配减少动画与三平台 | 跨平台离线歌词播放 | 11.3, 12.3, 12.4, 13.5 | React/Native | `tests/native/IL-R09-S02.md` | `pnpm verify:scenario -- IL-R09-S02` |

## library-experience

| Scenario ID | Requirement | Scenario | 任务 | 测试层 | 测试/步骤 manifest | 实际验收命令 |
|---|---|---|---|---|---|---|
| LE-R01-S01 | 资料库视图 | 切换全部歌曲 | 6.1, 10.5 | Core/React | `tests/scenarios/LE-R01-S01.yaml` | `pnpm verify:scenario -- LE-R01-S01` |
| LE-R01-S02 | 资料库视图 | 切换最近添加 | 6.1, 10.5 | Core/React | `tests/scenarios/LE-R01-S02.yaml` | `pnpm verify:scenario -- LE-R01-S02` |
| LE-R01-S03 | 资料库视图 | 切换喜欢的音乐 | 6.1, 10.5 | Core/React | `tests/scenarios/LE-R01-S03.yaml` | `pnpm verify:scenario -- LE-R01-S03` |
| LE-R02-S01 | 资料库搜索 | 搜索多个字段 | 3.6, 3.7, 6.2, 10.5 | Core/React/Perf | `tests/scenarios/LE-R02-S01.yaml` | `pnpm verify:scenario -- LE-R02-S01` |
| LE-R02-S02 | 资料库搜索 | 搜索词为空 | 3.6, 3.7, 6.2, 10.5 | Core/React/Perf | `tests/scenarios/LE-R02-S02.yaml` | `pnpm verify:scenario -- LE-R02-S02` |
| LE-R02-S03 | 资料库搜索 | 搜索无结果 | 3.6, 3.7, 6.2, 10.5 | Core/React/Perf | `tests/scenarios/LE-R02-S03.yaml` | `pnpm verify:scenario -- LE-R02-S03` |
| LE-R03-S01 | 全部歌曲排序 | 选择排序字段 | 3.8, 6.1, 10.5 | Core/React | `tests/scenarios/LE-R03-S01.yaml` | `pnpm verify:scenario -- LE-R03-S01` |
| LE-R03-S02 | 全部歌曲排序 | 排序值相同 | 3.8, 6.1, 10.5 | Core/React | `tests/scenarios/LE-R03-S02.yaml` | `pnpm verify:scenario -- LE-R03-S02` |
| LE-R03-S03 | 全部歌曲排序 | 非全部歌曲视图排序 | 3.8, 6.1, 10.5 | Core/React | `tests/scenarios/LE-R03-S03.yaml` | `pnpm verify:scenario -- LE-R03-S03` |
| LE-R04-S01 | 歌曲收藏 | 收藏歌曲 | 6.3, 10.6 | Core/React | `tests/scenarios/LE-R04-S01.yaml` | `pnpm verify:scenario -- LE-R04-S01` |
| LE-R04-S02 | 歌曲收藏 | 取消收藏 | 6.3, 10.6 | Core/React | `tests/scenarios/LE-R04-S02.yaml` | `pnpm verify:scenario -- LE-R04-S02` |
| LE-R05-S01 | 歌曲操作与详情 | 打开歌曲操作菜单 | 6.4, 8.11, 10.6, 10.8 | Core/Desktop/React/Native | `tests/native/LE-R05-S01.md` | `pnpm verify:scenario -- LE-R05-S01` |
| LE-R05-S02 | 歌曲操作与详情 | 查看歌曲详情 | 6.4, 8.11, 10.6, 10.8 | Core/Desktop/React/Native | `tests/native/LE-R05-S02.md` | `pnpm verify:scenario -- LE-R05-S02` |
| LE-R05-S03 | 歌曲操作与详情 | 删除当前播放歌曲 | 6.4, 8.11, 10.6, 10.8 | Core/Desktop/React/Native | `tests/native/LE-R05-S03.md` | `pnpm verify:scenario -- LE-R05-S03` |
| LE-R05-S04 | 歌曲操作与详情 | 打开本地目录 | 6.4, 8.11, 10.6, 10.8, 9.5 | Core/Desktop/React/Native | `tests/native/LE-R05-S04.md` | `pnpm verify:scenario -- LE-R05-S04` |
| LE-R05-S05 | 歌曲操作与详情 | 删除歌曲 | 6.4, 8.11, 10.6, 10.8 | Core/Desktop/React/Native | `tests/native/LE-R05-S05.md` | `pnpm verify:scenario -- LE-R05-S05` |
| LE-R05-S06 | 歌曲操作与详情 | 回收站结果无法证明 | 6.4, 8.11, 10.6, 10.8, 5.8, 13.3 | Core/Desktop/React/Native | `tests/native/LE-R05-S06.md` | `pnpm verify:scenario -- LE-R05-S06` |
| LE-R06-S01 | 资料库状态反馈 | 初次加载 | 4.10, 10.3, 10.7 | Core/React | `tests/scenarios/LE-R06-S01.yaml` | `pnpm verify:scenario -- LE-R06-S01` |
| LE-R06-S02 | 资料库状态反馈 | 资料库为空 | 4.10, 10.3, 10.7 | Core/React | `tests/scenarios/LE-R06-S02.yaml` | `pnpm verify:scenario -- LE-R06-S02` |
| LE-R06-S03 | 资料库状态反馈 | 资料库不可用 | 4.10, 10.3, 10.7 | Core/React | `tests/scenarios/LE-R06-S03.yaml` | `pnpm verify:scenario -- LE-R06-S03` |
| LE-R06-S04 | 资料库状态反馈 | 加载或扫描失败 | 4.10, 10.3, 10.7 | Core/React | `tests/scenarios/LE-R06-S04.yaml` | `pnpm verify:scenario -- LE-R06-S04` |
| LE-R07-S01 | 大曲库浏览 | 浏览大量歌曲 | 3.8, 10.6, 12.5 | Core/React/Perf | `tests/scenarios/LE-R07-S01.yaml` | `pnpm verify:scenario -- LE-R07-S01` |
| LE-R07-S02 | 大曲库浏览 | 搜索或排序后保持定位 | 3.8, 10.6, 12.5 | Core/React/Perf | `tests/scenarios/LE-R07-S02.yaml` | `pnpm verify:scenario -- LE-R07-S02` |

## local-library

| Scenario ID | Requirement | Scenario | 任务 | 测试层 | 测试/步骤 manifest | 实际验收命令 |
|---|---|---|---|---|---|---|
| LL-R01-S01 | 单一资料库根目录与本地持久化 | 选择根目录并建立资料库 | 3.1, 4.1, 10.3 | Core/Desktop/E2E | `tests/scenarios/LL-R01-S01.yaml` | `pnpm verify:scenario -- LL-R01-S01` |
| LL-R01-S02 | 单一资料库根目录与本地持久化 | 安全切换活动根目录 | 3.1, 4.1, 10.3 | Core/Desktop/E2E | `tests/scenarios/LL-R01-S02.yaml` | `pnpm verify:scenario -- LL-R01-S02` |
| LL-R01-S03 | 单一资料库根目录与本地持久化 | 切换根目录失败 | 3.1, 4.1, 10.3 | Core/Desktop/E2E | `tests/scenarios/LL-R01-S03.yaml` | `pnpm verify:scenario -- LL-R01-S03` |
| LL-R02-S01 | 扫描、监听与手动重扫 | 首次扫描发现支持文件 | 4.7, 4.9, 4.10 | Core/Infrastructure | `tests/scenarios/LL-R02-S01.yaml` | `pnpm verify:scenario -- LL-R02-S01` |
| LL-R02-S02 | 扫描、监听与手动重扫 | 忽略不支持文件 | 4.7, 4.9, 4.10 | Core/Infrastructure | `tests/scenarios/LL-R02-S02.yaml` | `pnpm verify:scenario -- LL-R02-S02` |
| LL-R02-S03 | 扫描、监听与手动重扫 | 监听外部新增和修改 | 4.7, 4.9, 4.10 | Core/Infrastructure | `tests/scenarios/LL-R02-S03.yaml` | `pnpm verify:scenario -- LL-R02-S03` |
| LL-R03-S01 | 一期格式与内容解析矩阵 | 解析内嵌数据 | 1.6, 4.3–4.5, 8.12 | Core/Desktop/Native | `tests/native/LL-R03-S01.md` | `pnpm verify:scenario -- LL-R03-S01` |
| LL-R03-S02 | 一期格式与内容解析矩阵 | 解析同名 LRC 侧车 | 1.6, 4.3–4.5, 8.12 | Core/Desktop/Native | `tests/native/LL-R03-S02.md` | `pnpm verify:scenario -- LL-R03-S02` |
| LL-R03-S03 | 一期格式与内容解析矩阵 | 文件损坏或标签异常 | 1.6, 4.3–4.5, 8.12 | Core/Desktop/Native | `tests/native/LL-R03-S03.md` | `pnpm verify:scenario -- LL-R03-S03` |
| LL-R04-S01 | 稳定身份、哈希与路径重关联 | 原地重扫保持身份 | 3.2, 4.8, 13.4 | Core/Infrastructure | `tests/scenarios/LL-R04-S01.yaml` | `pnpm verify:scenario -- LL-R04-S01` |
| LL-R04-S02 | 稳定身份、哈希与路径重关联 | 移动或改名后重关联 | 3.2, 4.8, 13.4 | Core/Infrastructure | `tests/scenarios/LL-R04-S02.yaml` | `pnpm verify:scenario -- LL-R04-S02` |
| LL-R04-S03 | 稳定身份、哈希与路径重关联 | 相同内容出现在两个路径 | 3.2, 4.8, 13.4 | Core/Infrastructure | `tests/scenarios/LL-R04-S03.yaml` | `pnpm verify:scenario -- LL-R04-S03` |
| LL-R04-S04 | 稳定身份、哈希与路径重关联 | 唯一音乐键弱重关联 | 3.2, 4.8, 13.4 | Core/Infrastructure | `tests/scenarios/LL-R04-S04.yaml` | `pnpm verify:scenario -- LL-R04-S04` |
| LL-R05-S01 | SQLite 检索与曲库排序 | 搜索并排序 | 3.1–3.8 | Infrastructure | `tests/scenarios/LL-R05-S01.yaml` | `pnpm verify:scenario -- LL-R05-S01` |
| LL-R05-S02 | SQLite 检索与曲库排序 | 重启后数据可用 | 3.1–3.8 | Infrastructure | `tests/scenarios/LL-R05-S02.yaml` | `pnpm verify:scenario -- LL-R05-S02` |
| LL-R06-S01 | 生效元数据、封面和歌词优先级 | 覆盖层优先展示 | 2.2, 4.5, 4.6 | Core/Infrastructure | `tests/scenarios/LL-R06-S01.yaml` | `pnpm verify:scenario -- LL-R06-S01` |
| LL-R06-S02 | 生效元数据、封面和歌词优先级 | 外部修改不覆盖用户值 | 2.2, 4.5, 4.6 | Core/Infrastructure | `tests/scenarios/LL-R06-S02.yaml` | `pnpm verify:scenario -- LL-R06-S02` |
| LL-R07-S01 | 资料库不可用与删除恢复 | 根目录不可用 | 5.7–5.10, 8.11, 10.3, 10.8 | Core/Desktop/React/Native | `tests/native/LL-R07-S01.md` | `pnpm verify:scenario -- LL-R07-S01` |
| LL-R07-S02 | 资料库不可用与删除恢复 | 删除歌曲后关联可见 | 5.7–5.10, 8.11, 10.3, 10.8 | Core/Desktop/React/Native | `tests/native/LL-R07-S02.md` | `pnpm verify:scenario -- LL-R07-S02` |
| LL-R07-S03 | 资料库不可用与删除恢复 | 文件恢复 | 5.7–5.10, 8.11, 10.3, 10.8 | Core/Desktop/React/Native | `tests/native/LL-R07-S03.md` | `pnpm verify:scenario -- LL-R07-S03` |
| LL-R08-S01 | 跨平台路径、隐私与性能约束 | Unicode 和平台路径 | 4.2, 7.7, 12.5–12.7, 13.4 | Security/Perf/Native | `tests/native/LL-R08-S01.md` | `pnpm verify:scenario -- LL-R08-S01` |
| LL-R08-S02 | 跨平台路径、隐私与性能约束 | 扫描期间继续使用界面 | 4.2, 7.7, 12.5–12.7, 13.4 | Security/Perf/Native | `tests/native/LL-R08-S02.md` | `pnpm verify:scenario -- LL-R08-S02` |
| LL-R08-S03 | 跨平台路径、隐私与性能约束 | 不泄露本机路径 | 4.2, 7.7, 12.5–12.7, 13.4, 1.8, 7.8 | Security/Perf/Native | `tests/native/LL-R08-S03.md` | `pnpm verify:scenario -- LL-R08-S03` |

## playlist-management

| Scenario ID | Requirement | Scenario | 任务 | 测试层 | 测试/步骤 manifest | 实际验收命令 |
|---|---|---|---|---|---|---|
| PM-R01-S01 | 歌单 CRUD | 创建歌单 | 6.5, 10.9 | Core/React | `tests/scenarios/PM-R01-S01.yaml` | `pnpm verify:scenario -- PM-R01-S01` |
| PM-R01-S02 | 歌单 CRUD | 名称无效 | 6.5, 10.9 | Core/React | `tests/scenarios/PM-R01-S02.yaml` | `pnpm verify:scenario -- PM-R01-S02` |
| PM-R01-S03 | 歌单 CRUD | 重命名歌单 | 6.5, 10.9 | Core/React | `tests/scenarios/PM-R01-S03.yaml` | `pnpm verify:scenario -- PM-R01-S03` |
| PM-R01-S04 | 歌单 CRUD | 删除歌单 | 6.5, 10.9 | Core/React | `tests/scenarios/PM-R01-S04.yaml` | `pnpm verify:scenario -- PM-R01-S04` |
| PM-R02-S01 | 歌单查看与成员顺序 | 打开歌单 | 3.2, 6.1, 6.6, 10.9 | Core/React | `tests/scenarios/PM-R02-S01.yaml` | `pnpm verify:scenario -- PM-R02-S01` |
| PM-R02-S02 | 歌单查看与成员顺序 | 追加后查看 | 3.2, 6.1, 6.6, 10.9 | Core/React | `tests/scenarios/PM-R02-S02.yaml` | `pnpm verify:scenario -- PM-R02-S02` |
| PM-R03-S01 | 歌单成员添加 | 添加单首歌曲 | 6.6, 10.9 | Core/React | `tests/scenarios/PM-R03-S01.yaml` | `pnpm verify:scenario -- PM-R03-S01` |
| PM-R03-S02 | 歌单成员添加 | 添加到多个歌单 | 6.6, 10.9 | Core/React | `tests/scenarios/PM-R03-S02.yaml` | `pnpm verify:scenario -- PM-R03-S02` |
| PM-R03-S03 | 歌单成员添加 | 重复添加 | 6.6, 10.9 | Core/React | `tests/scenarios/PM-R03-S03.yaml` | `pnpm verify:scenario -- PM-R03-S03` |
| PM-R04-S01 | 歌单成员移除 | 移除成员 | 6.6, 10.9 | Core/React | `tests/scenarios/PM-R04-S01.yaml` | `pnpm verify:scenario -- PM-R04-S01` |
| PM-R05-S01 | 失效歌曲成员 | 文件暂时不可用 | 5.9, 6.7, 10.9 | Core/React | `tests/scenarios/PM-R05-S01.yaml` | `pnpm verify:scenario -- PM-R05-S01` |
| PM-R05-S02 | 失效歌曲成员 | 文件已删除 | 5.9, 6.7, 10.9 | Core/React | `tests/scenarios/PM-R05-S02.yaml` | `pnpm verify:scenario -- PM-R05-S02` |
| PM-R05-S03 | 失效歌曲成员 | 用户在 Echo 中删除歌曲 | 5.9, 6.7, 10.9 | Core/React | `tests/scenarios/PM-R05-S03.yaml` | `pnpm verify:scenario -- PM-R05-S03` |
| PM-R05-S04 | 失效歌曲成员 | 失效歌曲恢复 | 5.9, 6.7, 10.9 | Core/React | `tests/scenarios/PM-R05-S04.yaml` | `pnpm verify:scenario -- PM-R05-S04` |

## safe-file-ingestion

| Scenario ID | Requirement | Scenario | 任务 | 测试层 | 测试/步骤 manifest | 实际验收命令 |
|---|---|---|---|---|---|---|
| SFI-R01-S01 | 多选导入与默认目标命名 | 多选文件成功导入 | 5.1, 5.2, 10.10 | Core/React/E2E | `tests/scenarios/SFI-R01-S01.yaml` | `pnpm verify:scenario -- SFI-R01-S01` |
| SFI-R01-S02 | 多选导入与默认目标命名 | 标签缺失和非法字符 | 5.1, 5.2, 10.10 | Core/React/E2E | `tests/scenarios/SFI-R01-S02.yaml` | `pnpm verify:scenario -- SFI-R01-S02` |
| SFI-R02-S01 | 同名歌词侧车导入 | 音频与 LRC 一起导入 | 5.4, 10.10 | Core/React | `tests/scenarios/SFI-R02-S01.yaml` | `pnpm verify:scenario -- SFI-R02-S01` |
| SFI-R02-S02 | 同名歌词侧车导入 | LRC 不可读 | 5.4, 10.10 | Core/React | `tests/scenarios/SFI-R02-S02.yaml` | `pnpm verify:scenario -- SFI-R02-S02` |
| SFI-R03-S01 | BLAKE3 去重与重名编号 | 内容重复 | 5.2, 5.6 | Core/Infrastructure | `tests/scenarios/SFI-R03-S01.yaml` | `pnpm verify:scenario -- SFI-R03-S01` |
| SFI-R03-S02 | BLAKE3 去重与重名编号 | 同名不同内容 | 5.2, 5.6 | Core/Infrastructure | `tests/scenarios/SFI-R03-S02.yaml` | `pnpm verify:scenario -- SFI-R03-S02` |
| SFI-R04-S01 | 暂存、校验、原子移动与操作日志 | 校验失败不落库 | 5.3–5.5, 13.3 | Core/Fault injection | `tests/scenarios/SFI-R04-S01.yaml` | `pnpm verify:scenario -- SFI-R04-S01` |
| SFI-R04-S02 | 暂存、校验、原子移动与操作日志 | 导入成功原子可见 | 5.3–5.5, 13.3 | Core/Fault injection | `tests/scenarios/SFI-R04-S02.yaml` | `pnpm verify:scenario -- SFI-R04-S02` |
| SFI-R04-S03 | 暂存、校验、原子移动与操作日志 | 崩溃后恢复 | 5.3–5.5, 13.3 | Core/Fault injection | `tests/scenarios/SFI-R04-S03.yaml` | `pnpm verify:scenario -- SFI-R04-S03` |
| SFI-R04-S04 | 暂存、校验、原子移动与操作日志 | 音频发布后侧车失败 | 5.3–5.5, 13.3 | Core/Fault injection | `tests/scenarios/SFI-R04-S04.yaml` | `pnpm verify:scenario -- SFI-R04-S04` |
| SFI-R04-S05 | 暂存、校验、原子移动与操作日志 | 发布后 watcher 抢先观察 | 5.3–5.5, 13.3, 4.9, 5.5 | Core/Fault injection | `tests/scenarios/SFI-R04-S05.yaml` | `pnpm verify:scenario -- SFI-R04-S05` |
| SFI-R05-S01 | 逐文件结果与资料库不可用反馈 | 混合结果 | 5.1, 10.3, 10.10 | Core/React | `tests/scenarios/SFI-R05-S01.yaml` | `pnpm verify:scenario -- SFI-R05-S01` |
| SFI-R05-S02 | 逐文件结果与资料库不可用反馈 | 导入时根目录断开 | 5.1, 10.3, 10.10 | Core/React | `tests/scenarios/SFI-R05-S02.yaml` | `pnpm verify:scenario -- SFI-R05-S02` |
| SFI-R06-S01 | 源文件、系统关联与安全边界 | 外部文件直接打开 | 4.2, 7.5, 9.2, 12.7 | Security/Native | `tests/native/SFI-R06-S01.md` | `pnpm verify:scenario -- SFI-R06-S01` |
| SFI-R06-S02 | 源文件、系统关联与安全边界 | 活动资料库内文件直接打开 | 4.2, 7.5, 9.2, 12.7 | Security/Native | `tests/native/SFI-R06-S02.md` | `pnpm verify:scenario -- SFI-R06-S02` |
| SFI-R06-S03 | 源文件、系统关联与安全边界 | 非活动旧资料库文件直接打开 | 4.2, 7.5, 9.2, 12.7, 8.9 | Security/Native | `tests/native/SFI-R06-S03.md` | `pnpm verify:scenario -- SFI-R06-S03` |
| SFI-R06-S04 | 源文件、系统关联与安全边界 | 源文件保持不变 | 4.2, 7.5, 9.2, 12.7, 5.3 | Security/Native | `tests/native/SFI-R06-S04.md` | `pnpm verify:scenario -- SFI-R06-S04` |
| SFI-R06-S05 | 源文件、系统关联与安全边界 | 暂存目录名称与用户内容冲突 | 4.2, 7.5, 9.2, 12.7, 5.3, 5.7 | Security/Native | `tests/native/SFI-R06-S05.md` | `pnpm verify:scenario -- SFI-R06-S05` |
| SFI-R07-S01 | 单实例唤醒与重复打开 | 已运行实例接收文件关联 | 9.1, 9.2 | Desktop/Native | `tests/native/SFI-R07-S01.md` | `pnpm verify:scenario -- SFI-R07-S01` |
| SFI-R07-S02 | 单实例唤醒与重复打开 | 冷启动文件关联 | 9.1, 9.2 | Desktop/Native | `tests/native/SFI-R07-S02.md` | `pnpm verify:scenario -- SFI-R07-S02` |
| SFI-R08-S01 | 跨平台路径与恢复后的幂等性 | 跨平台安全命名 | 5.5, 5.10, 13.3, 13.4 | Fault injection/Native | `tests/native/SFI-R08-S01.md` | `pnpm verify:scenario -- SFI-R08-S01` |
| SFI-R08-S02 | 跨平台路径与恢复后的幂等性 | 重试导入幂等 | 5.5, 5.10, 13.3, 13.4 | Fault injection/Native | `tests/native/SFI-R08-S02.md` | `pnpm verify:scenario -- SFI-R08-S02` |

## 发布审计

1. 运行 `pnpm verify:scenario -- --all`，保存逐场景结果与集合差异报告。
2. P0 文件安全、恢复、路径边界与回收站场景必须是自动故障注入，不得只用人工步骤。
3. 执行 PRD A1–A14、完整质量命令和三平台原生矩阵；任何缺失映射或 P0 失败阻断 0.1.0。
