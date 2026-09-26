# 0.1.0 场景级需求追踪

## 完成规则

- 本表逐项覆盖 specs 中的全部 Scenario；稳定 ID 不得复用，标题调整时保留原 ID。
- 任务 1.3 建立测试 manifest 与执行器。每行指定的 manifest 必须包含正常路径、关键失败路径、fixture、预期结果和自动测试过滤器；Native 行还必须包含平台版本、人工步骤、操作者和证据路径。
- 每行给出实际验收命令。`pnpm verify:scenario -- --all` 必须比较 specs、此表和测试 manifest 的 ID 集合完全相等，并逐项执行；缺失、重复、无命令、无证据或失败均返回非零。
- 当前基线：102 个 Requirement，392 个 Scenario（0.1.0 归档后含 sync-foundation 6、原生对话框 5、导航计数 8、歌单搜索/定位/导入入口 24、三平台许可证齐全 1、平台播放后端缺失 1）。Requirement 计数只用于审计，不再用整行继承代替场景映射。

## ci-release-pipeline

| Scenario ID | Requirement | Scenario | 任务 | 测试层 | 测试/步骤 manifest | 实际验收命令 |
|---|---|---|---|---|---|---|
| CRP-R01-S01 | API tag 触发发布 | 语义化 tag 触发构建 | 13.9 | Gate | `tests/scenarios/CRP-R01-S01.yaml` | `pnpm verify:scenario -- CRP-R01-S01` |
| CRP-R01-S02 | API tag 触发发布 | 版本不匹配时拒绝发布 | 13.9 | Gate | `tests/scenarios/CRP-R01-S02.yaml` | `pnpm verify:scenario -- CRP-R01-S02` |
| CRP-R01-S03 | API tag 触发发布 | 手动触发 | 13.9 | Gate | `tests/scenarios/CRP-R01-S03.yaml` | `pnpm verify:scenario -- CRP-R01-S03` |
| CRP-R02-S01 | 三平台产物矩阵 | macOS 产物 | 13.9 | Gate | `tests/scenarios/CRP-R02-S01.yaml` | `pnpm verify:scenario -- CRP-R02-S01` |
| CRP-R02-S02 | 三平台产物矩阵 | Windows 产物 | 13.9 | Gate | `tests/scenarios/CRP-R02-S02.yaml` | `pnpm verify:scenario -- CRP-R02-S02` |
| CRP-R02-S03 | 三平台产物矩阵 | Linux 产物 | 13.9 | Gate | `tests/scenarios/CRP-R02-S03.yaml` | `pnpm verify:scenario -- CRP-R02-S03` |
| CRP-R03-S01 | 产物完整性校验 | 校验清单随发布物上传 | 13.9 | Gate | `tests/scenarios/CRP-R03-S01.yaml` | `pnpm verify:scenario -- CRP-R03-S01` |
| CRP-R03-S02 | 产物完整性校验 | 缺失产物导致失败 | 13.9 | Gate | `tests/scenarios/CRP-R03-S02.yaml` | `pnpm verify:scenario -- CRP-R03-S02` |
| CRP-R04-S01 | 第三方许可证随发布物提供 | 许可证说明随包上传 | 13.9 | Gate | `tests/scenarios/CRP-R04-S01.yaml` | `pnpm verify:scenario -- CRP-R04-S01` |
| CRP-R04-S02 | 第三方许可证随发布物提供 | 三平台许可齐全 | 13.9 | Gate | `tests/scenarios/CRP-R04-S02.yaml` | `pnpm verify:scenario -- CRP-R04-S02` |
| CRP-R05-S01 | 发布为 GitHub Release | 资产汇总到 tag 对应 Release | 13.9 | Gate | `tests/scenarios/CRP-R05-S01.yaml` | `pnpm verify:scenario -- CRP-R05-S01` |
| CRP-R05-S02 | 发布为 GitHub Release | 重复触发不产生重复 Release | 13.9 | Gate | `tests/scenarios/CRP-R05-S02.yaml` | `pnpm verify:scenario -- CRP-R05-S02` |

## desktop-app-shell

| Scenario ID | Requirement | Scenario | 任务 | 测试层 | 测试/步骤 manifest | 实际验收命令 |
|---|---|---|---|---|---|---|
| DAS-R01-S01 | 首次启动必须建立本机工作区 | 首次选择有效目录 | 4.1, 7.1, 10.3, 13.2 | Core/React/E2E | `tests/scenarios/DAS-R01-S01.yaml` | `pnpm verify:scenario -- DAS-R01-S01` |
| DAS-R01-S02 | 首次启动必须建立本机工作区 | 取消选择目录 | 4.1, 7.1, 10.3, 13.2 | Core/React/E2E | `tests/scenarios/DAS-R01-S02.yaml` | `pnpm verify:scenario -- DAS-R01-S02` |
| DAS-R01-S03 | 首次启动必须建立本机工作区 | 选择不可用目录 | 4.1, 7.1, 10.3, 13.2 | Core/React/E2E | `tests/scenarios/DAS-R01-S03.yaml` | `pnpm verify:scenario -- DAS-R01-S03` |
| DAS-R01-S04 | 首次启动必须建立本机工作区 | 已有本机配置再次启动 | 4.1, 7.1, 10.3, 13.2 | Core/React/E2E | `tests/scenarios/DAS-R01-S04.yaml` | `pnpm verify:scenario -- DAS-R01-S04` |
| DAS-R02-S01 | 应用框架必须提供稳定的目录入口 | 从侧边导航切换资料库视图 | 10.4, 12.3 | React/E2E | `tests/scenarios/DAS-R02-S01.yaml` | `pnpm verify:scenario -- DAS-R02-S01` |
| DAS-R02-S02 | 应用框架必须提供稳定的目录入口 | 视图或资料库为空 | 10.4, 12.3 | React/E2E | `tests/scenarios/DAS-R02-S02.yaml` | `pnpm verify:scenario -- DAS-R02-S02` |
| DAS-R02-S03 | 应用框架必须提供稳定的目录入口 | 进入多选模式 | 10.4, 12.3 | React/E2E | `tests/scenarios/DAS-R02-S03.yaml` | `pnpm verify:scenario -- DAS-R02-S03` |
| DAS-R02-S04 | 应用框架必须提供稳定的目录入口 | 右键和键盘打开批量菜单 | 4.1, 7.1, 10.3, 13.2 | Core/React/E2E | `tests/scenarios/DAS-R02-S04.yaml` | `pnpm verify:scenario -- DAS-R02-S04` |
| DAS-R02-S05 | 应用框架必须提供稳定的目录入口 | 访问一期明确排除的入口 | 10.4, 12.3 | React/E2E | `tests/scenarios/DAS-R02-S05.yaml` | `pnpm verify:scenario -- DAS-R02-S05` |
| DAS-R02-S06 | 应用框架必须提供稳定的目录入口 | 悬停 Echo 品牌区域 | 4.1, 7.1, 10.3, 13.2 | Native/Gate | `tests/native/DAS-R02-S06.md` | `pnpm verify:scenario -- DAS-R02-S06` |
| DAS-R02-S07 | 应用框架必须提供稳定的目录入口 | 指针离开品牌区域 | 4.1, 7.1, 10.3, 13.2 | Native/Gate | `tests/native/DAS-R02-S07.md` | `pnpm verify:scenario -- DAS-R02-S07` |
| DAS-R02-S08 | 应用框架必须提供稳定的目录入口 | 键盘访问品牌操作 | 4.1, 7.1, 10.3, 13.2 | Core/React/E2E | `tests/scenarios/DAS-R02-S08.yaml` | `pnpm verify:scenario -- DAS-R02-S08` |
| DAS-R02-S09 | 应用框架必须提供稳定的目录入口 | 便于点击品牌操作 | 4.1, 7.1, 10.3, 13.2 | Native/Gate | `tests/native/DAS-R02-S09.md` | `pnpm verify:scenario -- DAS-R02-S09` |
| DAS-R02-S10 | 应用框架必须提供稳定的目录入口 | 保持搜索框外观 | 4.1, 7.1, 10.3, 13.2 | Native/Gate | `tests/native/DAS-R02-S10.md` | `pnpm verify:scenario -- DAS-R02-S10` |
| DAS-R03-S01 | 品牌区导入入口所有视图可用 | 歌单视图导入可用 | 5.1, 5.2 | React | `tests/scenarios/DAS-R03-S01.yaml` | `pnpm verify:scenario -- DAS-R03-S01` |
| DAS-R03-S02 | 品牌区导入入口所有视图可用 | 歌手/专辑视图导入可用 | 5.1, 5.2 | React | `tests/scenarios/DAS-R03-S02.yaml` | `pnpm verify:scenario -- DAS-R03-S02` |
| DAS-R03-S03 | 品牌区导入入口所有视图可用 | 只读资目录禁用导入 | 5.2 | React | `tests/scenarios/DAS-R03-S03.yaml` | `pnpm verify:scenario -- DAS-R03-S03` |
| DAS-R04-S01 | 主题与本机偏好必须持久化且可回退 | 切换主题 | 7.6, 10.1 | Desktop/React | `tests/scenarios/DAS-R04-S01.yaml` | `pnpm verify:scenario -- DAS-R04-S01` |
| DAS-R04-S02 | 主题与本机偏好必须持久化且可回退 | 下次启动恢复主题 | 7.6, 10.1 | Desktop/React | `tests/scenarios/DAS-R04-S02.yaml` | `pnpm verify:scenario -- DAS-R04-S02` |
| DAS-R04-S03 | 主题与本机偏好必须持久化且可回退 | 偏好不可保存或值非法 | 7.6, 10.1 | Desktop/React | `tests/scenarios/DAS-R04-S03.yaml` | `pnpm verify:scenario -- DAS-R04-S03` |
| DAS-R05-S01 | 主窗口关闭行为必须符合用户选择并保留播放状态 | 选择退出应用 | 7.6, 9.6, 13.5 | Desktop/Native | `tests/native/DAS-R05-S01.md` | `pnpm verify:scenario -- DAS-R05-S01` |
| DAS-R05-S02 | 主窗口关闭行为必须符合用户选择并保留播放状态 | 选择后台运行 | 7.6, 9.6, 13.5 | Desktop/Native | `tests/native/DAS-R05-S02.md` | `pnpm verify:scenario -- DAS-R05-S02` |
| DAS-R05-S03 | 主窗口关闭行为必须符合用户选择并保留播放状态 | 关闭期间存在未完成初始化 | 7.6, 9.6, 13.5 | Desktop/Native | `tests/native/DAS-R05-S03.md` | `pnpm verify:scenario -- DAS-R05-S03` |
| DAS-R06-S01 | 系统托盘与菜单栏入口必须提供一致的后台控制 | 从平台入口显示主窗口 | 9.3, 13.5 | Desktop/Native | `tests/native/DAS-R06-S01.md` | `pnpm verify:scenario -- DAS-R06-S01` |
| DAS-R06-S02 | 系统托盘与菜单栏入口必须提供一致的后台控制 | 从平台入口控制播放 | 9.3, 13.5 | Desktop/Native | `tests/native/DAS-R06-S02.md` | `pnpm verify:scenario -- DAS-R06-S02` |
| DAS-R06-S03 | 系统托盘与菜单栏入口必须提供一致的后台控制 | 托盘或菜单栏初始化失败 | 9.3, 13.5 | Desktop/Native | `tests/native/DAS-R06-S03.md` | `pnpm verify:scenario -- DAS-R06-S03` |
| DAS-R07-S01 | 应用必须保证单实例与文件打开唤醒 | 重复启动应用 | 7.1, 9.1, 9.2 | Desktop/Native | `tests/native/DAS-R07-S01.md` | `pnpm verify:scenario -- DAS-R07-S01` |
| DAS-R07-S02 | 应用必须保证单实例与文件打开唤醒 | 系统文件关联触发打开 | 7.1, 9.1, 9.2 | Desktop/Native | `tests/native/DAS-R07-S02.md` | `pnpm verify:scenario -- DAS-R07-S02` |
| DAS-R07-S03 | 应用必须保证单实例与文件打开唤醒 | 首实例尚未就绪 | 7.1, 9.1, 9.2 | Desktop/Native | `tests/native/DAS-R07-S03.md` | `pnpm verify:scenario -- DAS-R07-S03` |
| DAS-R07-S04 | 应用必须保证单实例与文件打开唤醒 | 前端监听器晚于文件打开请求就绪 | — | Gate | `tests/native/DAS-R07-S04.md` | `pnpm verify:scenario -- DAS-R07-S04` |
| DAS-R07-S05 | 应用必须保证单实例与文件打开唤醒 | 运行中收到文件打开请求 | — | Gate | `tests/native/DAS-R07-S05.md` | `pnpm verify:scenario -- DAS-R07-S05` |
| DAS-R08-S01 | 窄屏布局与浮层关闭必须可预测 | 打开和关闭窄屏侧边栏 | 10.4, 12.1, 12.3 | React/E2E | `tests/scenarios/DAS-R08-S01.yaml` | `pnpm verify:scenario -- DAS-R08-S01` |
| DAS-R08-S02 | 窄屏布局与浮层关闭必须可预测 | 窗口从宽屏变为窄屏 | 10.4, 12.1, 12.3 | React/E2E | `tests/scenarios/DAS-R08-S02.yaml` | `pnpm verify:scenario -- DAS-R08-S02` |
| DAS-R08-S03 | 窄屏布局与浮层关闭必须可预测 | 浮层点击外部 | 10.4, 12.1, 12.3 | React/E2E | `tests/scenarios/DAS-R08-S03.yaml` | `pnpm verify:scenario -- DAS-R08-S03` |
| DAS-R09-S01 | 键盘焦点与 Escape 行为必须可访问 | 键盘遍历应用壳 | 11.8, 12.1, 12.2 | React/E2E | `tests/scenarios/DAS-R09-S01.yaml` | `pnpm verify:scenario -- DAS-R09-S01` |
| DAS-R09-S02 | 键盘焦点与 Escape 行为必须可访问 | Escape 关闭最上层浮层 | 11.8, 12.1, 12.2 | React/E2E | `tests/scenarios/DAS-R09-S02.yaml` | `pnpm verify:scenario -- DAS-R09-S02` |
| DAS-R09-S03 | 键盘焦点与 Escape 行为必须可访问 | 使用辅助技术 | 11.8, 12.1, 12.2 | React/E2E | `tests/scenarios/DAS-R09-S03.yaml` | `pnpm verify:scenario -- DAS-R09-S03` |
| DAS-R10-S01 | 三平台壳行为必须可验证且不依赖网络 | 离线启动 | 1.5, 7.7, 9.8, 13.5, 13.7, 13.8 | Security/Native | `tests/native/DAS-R10-S01.md` | `pnpm verify:scenario -- DAS-R10-S01` |
| DAS-R10-S02 | 三平台壳行为必须可验证且不依赖网络 | 跨平台验证 | 1.5, 7.7, 9.8, 13.5, 13.7, 13.8 | Security/Native | `tests/native/DAS-R10-S02.md` | `pnpm verify:scenario -- DAS-R10-S02` |
| DAS-R11-S01 | 目录与导入文件选择必须由系统原生对话框提供 | 从欢迎界面打开原生目录选择器 | wire 1.1–1.8 | Desktop/Shell | `tests/scenarios/DAS-R11-S01.yaml` | `pnpm verify:scenario -- DAS-R11-S01` |
| DAS-R11-S02 | 目录与导入文件选择必须由系统原生对话框提供 | 取消原生目录选择 | wire 1.4 | Desktop/Shell | `tests/scenarios/DAS-R11-S02.yaml` | `pnpm verify:scenario -- DAS-R11-S02` |
| DAS-R11-S03 | 目录与导入文件选择必须由系统原生对话框提供 | WebView 不接触文件系统路径 | wire 1.7, 3.1 | Desktop/Shell | `tests/scenarios/DAS-R11-S03.yaml` | `pnpm verify:scenario -- DAS-R11-S03` |
| DAS-R12-S01 | 欢迎/初始化界面必须占满工作区 | 未配置时显示完整初始化页 | wire 2.1–2.2 | React | `tests/scenarios/DAS-R12-S01.yaml` | `pnpm verify:scenario -- DAS-R12-S01` |
| DAS-R12-S02 | 欢迎/初始化界面必须占满工作区 | 资料库不可用时显示完整状态页 | wire 2.1–2.2 | React | `tests/scenarios/DAS-R12-S02.yaml` | `pnpm verify:scenario -- DAS-R12-S02` |
| DAS-R13-S01 | 后台播放生命周期完整性 | 关闭窗口后继续后台播放 | — | Gate | `tests/scenarios/DAS-R13-S01.yaml` | `pnpm verify:scenario -- DAS-R13-S01` |
| DAS-R13-S02 | 后台播放生命周期完整性 | 从后台入口退出 | — | Gate | `tests/scenarios/DAS-R13-S02.yaml` | `pnpm verify:scenario -- DAS-R13-S02` |
| DAS-R14-S01 | Echo 应用名称一致性 | 可见应用身份一致 | — | Gate | `tests/scenarios/DAS-R14-S01.yaml` | `pnpm verify:scenario -- DAS-R14-S01` |
| DAS-R15-S01 | macOS 菜单栏传输控制与应用身份 | 从常驻菜单栏控制传输 | — | Desktop/Native | `tests/native/DAS-R15-S01.md` | `pnpm verify:scenario -- DAS-R15-S01` |
| DAS-R15-S02 | macOS 菜单栏传输控制与应用身份 | 紧凑控件按点击位置隔离动作 | — | Desktop/Native | `tests/native/DAS-R15-S02.md` | `pnpm verify:scenario -- DAS-R15-S02` |
| DAS-R15-S03 | macOS 菜单栏传输控制与应用身份 | 状态栏控制视觉验收 | — | Desktop/Native | `tests/native/DAS-R15-S03.md` | `pnpm verify:scenario -- DAS-R15-S03` |
| DAS-R15-S04 | macOS 菜单栏传输控制与应用身份 | Echo 图标打开单例窗口 | — | Desktop/Native | `tests/native/DAS-R15-S04.md` | `pnpm verify:scenario -- DAS-R15-S04` |
| DAS-R15-S05 | macOS 菜单栏传输控制与应用身份 | 重复点击不创建多个窗口 | — | Desktop/Native | `tests/native/DAS-R15-S05.md` | `pnpm verify:scenario -- DAS-R15-S05` |
| DAS-R15-S06 | macOS 菜单栏传输控制与应用身份 | 打包后的身份显示 | — | Desktop/Native | `tests/native/DAS-R15-S06.md` | `pnpm verify:scenario -- DAS-R15-S06` |
| DAS-R16-S01 | 应用禁用 WebView 默认右键菜单 | 在应用工作区右键 | 4.1, 7.1, 10.3, 13.2 | Native/Gate | `tests/native/DAS-R16-S01.md` | `pnpm verify:scenario -- DAS-R16-S01` |

## desktop-playback

| Scenario ID | Requirement | Scenario | 任务 | 测试层 | 测试/步骤 manifest | 实际验收命令 |
|---|---|---|---|---|---|---|
| DP-R01-S01 | 桌面音频播放 | 播放资料库歌曲 | 8.1–8.4, 8.12 | Desktop/Native | `tests/native/DP-R01-S01.md` | `pnpm verify:scenario -- DP-R01-S01` |
| DP-R01-S02 | 桌面音频播放 | 文件不可用 | 8.1–8.4, 8.12 | Desktop/Native | `tests/native/DP-R01-S02.md` | `pnpm verify:scenario -- DP-R01-S02` |
| DP-R01-S03 | 桌面音频播放 | 平台播放后端缺失 | 9.7 | Gate | `tests/scenarios/DP-R01-S03.yaml` | `pnpm verify:scenario -- DP-R01-S03` |
| DP-R02-S01 | 播放队列 | 从曲库开始播放 | 8.5, 8.6, 11.2 | Desktop/React | `tests/scenarios/DP-R02-S01.yaml` | `pnpm verify:scenario -- DP-R02-S01` |
| DP-R02-S02 | 播放队列 | 视图播放重建队列数量 | — | Gate | `tests/scenarios/DP-R02-S02.yaml` | `pnpm verify:scenario -- DP-R02-S02` |
| DP-R02-S03 | 播放队列 | 当前项始终置顶 | 8.5, 8.6, 11.2 | Desktop/React | `tests/scenarios/DP-R02-S03.yaml` | `pnpm verify:scenario -- DP-R02-S03` |
| DP-R02-S04 | 播放队列 | 加入队列 | 8.5, 8.6, 11.2 | Desktop/React | `tests/scenarios/DP-R02-S04.yaml` | `pnpm verify:scenario -- DP-R02-S04` |
| DP-R02-S05 | 播放队列 | 下一首播放 | — | Gate | `tests/scenarios/DP-R02-S05.yaml` | `pnpm verify:scenario -- DP-R02-S05` |
| DP-R02-S06 | 播放队列 | 连续下一首播放 | — | Gate | `tests/scenarios/DP-R02-S06.yaml` | `pnpm verify:scenario -- DP-R02-S06` |
| DP-R02-S07 | 播放队列 | 下一首播放同一首去重 | 8.5, 8.6, 11.2 | Desktop/React | `tests/scenarios/DP-R02-S07.yaml` | `pnpm verify:scenario -- DP-R02-S07` |
| DP-R02-S08 | 播放队列 | 提升不重复展开普通待播 | — | Gate | `tests/native/DP-R02-S08.md` | `pnpm verify:scenario -- DP-R02-S08` |
| DP-R02-S09 | 播放队列 | 优先区物理连续 | — | Gate | `tests/native/DP-R02-S09.md` | `pnpm verify:scenario -- DP-R02-S09` |
| DP-R02-S10 | 播放队列 | 优先区重复副本清理 | — | Gate | `tests/native/DP-R02-S10.md` | `pnpm verify:scenario -- DP-R02-S10` |
| DP-R02-S11 | 播放队列 | 去重判据不覆盖普通入队 | — | Gate | `tests/native/DP-R02-S11.md` | `pnpm verify:scenario -- DP-R02-S11` |
| DP-R02-S12 | 播放队列 | 优先区消费 | — | Gate | `tests/native/DP-R02-S12.md` | `pnpm verify:scenario -- DP-R02-S12` |
| DP-R02-S13 | 播放队列 | 清空队列 | — | Gate | `tests/native/DP-R02-S13.md` | `pnpm verify:scenario -- DP-R02-S13` |
| DP-R02-S14 | 播放队列 | 会话恢复的优先区 | — | Gate | `tests/native/DP-R02-S14.md` | `pnpm verify:scenario -- DP-R02-S14` |
| DP-R03-S01 | 播放队列面板展示 | 队列显示歌曲信息 | — | Gate | `tests/scenarios/DP-R03-S01.yaml` | `pnpm verify:scenario -- DP-R03-S01` |
| DP-R03-S02 | 播放队列面板展示 | 队列超过八首 | — | Gate | `tests/scenarios/DP-R03-S02.yaml` | `pnpm verify:scenario -- DP-R03-S02` |
| DP-R03-S03 | 播放队列面板展示 | 队列不超过八首 | — | Gate | `tests/scenarios/DP-R03-S03.yaml` | `pnpm verify:scenario -- DP-R03-S03` |
| DP-R03-S04 | 播放队列面板展示 | 沉浸模式尺寸一致 | — | Gate | `tests/scenarios/DP-R03-S04.yaml` | `pnpm verify:scenario -- DP-R03-S04` |
| DP-R04-S01 | 播放模式与传输控制 | 基础传输控制 | 8.6, 8.8, 11.1 | Desktop/React | `tests/scenarios/DP-R04-S01.yaml` | `pnpm verify:scenario -- DP-R04-S01` |
| DP-R04-S02 | 播放模式与传输控制 | 列表循环导航 | — | Gate | `tests/scenarios/DP-R04-S02.yaml` | `pnpm verify:scenario -- DP-R04-S02` |
| DP-R04-S03 | 播放模式与传输控制 | 随机播放导航 | — | Gate | `tests/scenarios/DP-R04-S03.yaml` | `pnpm verify:scenario -- DP-R04-S03` |
| DP-R04-S04 | 播放模式与传输控制 | 单曲循环的自然结束与手动下一首 | — | Gate | `tests/scenarios/DP-R04-S04.yaml` | `pnpm verify:scenario -- DP-R04-S04` |
| DP-R04-S05 | 播放模式与传输控制 | 切换播放模式 | 8.6, 8.8, 11.1 | Desktop/React | `tests/scenarios/DP-R04-S05.yaml` | `pnpm verify:scenario -- DP-R04-S05` |
| DP-R04-S06 | 播放模式与传输控制 | 定位和音量 | 8.6, 8.8, 11.1 | Desktop/React | `tests/scenarios/DP-R04-S06.yaml` | `pnpm verify:scenario -- DP-R04-S06` |
| DP-R05-S01 | 播放错误处理 | 单曲加载失败 | 8.7, 11.2 | Desktop/React | `tests/scenarios/DP-R05-S01.yaml` | `pnpm verify:scenario -- DP-R05-S01` |
| DP-R05-S02 | 播放错误处理 | 单曲循环中的加载失败 | 8.7, 11.2 | Desktop/React | `tests/scenarios/DP-R05-S02.yaml` | `pnpm verify:scenario -- DP-R05-S02` |
| DP-R05-S03 | 播放错误处理 | 资料库不可用期间播放 | 8.7, 11.2 | Desktop/React | `tests/scenarios/DP-R05-S03.yaml` | `pnpm verify:scenario -- DP-R05-S03` |
| DP-R06-S01 | 加载尝试必须重置进度事实 | 被拒绝的加载不残留上次时长 | — | Gate | `tests/scenarios/DP-R06-S01.yaml` | `pnpm verify:scenario -- DP-R06-S01` |
| DP-R06-S02 | 加载尝试必须重置进度事实 | 库内歌曲解析失败同样清空 | — | Gate | `tests/scenarios/DP-R06-S02.yaml` | `pnpm verify:scenario -- DP-R06-S02` |
| DP-R06-S03 | 加载尝试必须重置进度事实 | 进入 Loading 时不携带旧进度 | — | Gate | `tests/native/DP-R06-S03.md` | `pnpm verify:scenario -- DP-R06-S03` |
| DP-R06-S04 | 加载尝试必须重置进度事实 | 加载成功后的进度仍连续 | — | Gate | `tests/native/DP-R06-S04.md` | `pnpm verify:scenario -- DP-R06-S04` |
| DP-R07-S01 | 临时播放项边界 | 打开资料库外文件 | 9.2, 11.7 | Desktop/Native | `tests/native/DP-R07-S01.md` | `pnpm verify:scenario -- DP-R07-S01` |
| DP-R07-S02 | 临时播放项边界 | 临时项执行持久化操作 | 9.2, 11.7 | Desktop/Native | `tests/native/DP-R07-S02.md` | `pnpm verify:scenario -- DP-R07-S02` |
| DP-R07-S03 | 临时播放项边界 | 临时项导入后转换为库内歌曲 | 8.1–8.4, 8.12 | Desktop/Native | `tests/scenarios/DP-R07-S03.yaml` | `pnpm verify:scenario -- DP-R07-S03` |
| DP-R07-S04 | 临时播放项边界 | 临时项导入失败不改变播放身份 | 8.1–8.4, 8.12 | Desktop/Native | `tests/scenarios/DP-R07-S04.yaml` | `pnpm verify:scenario -- DP-R07-S04` |
| DP-R08-S01 | 播放统计 | 达到播放阈值 | 3.9, 8.10 | Core/Desktop | `tests/scenarios/DP-R08-S01.yaml` | `pnpm verify:scenario -- DP-R08-S01` |
| DP-R08-S02 | 播放统计 | 未达到阈值或 seek 作弊 | 3.9, 8.10 | Core/Desktop | `tests/scenarios/DP-R08-S02.yaml` | `pnpm verify:scenario -- DP-R08-S02` |
| DP-R09-S01 | 会话恢复 | 正常恢复 | 7.6, 8.9 | Desktop | `tests/scenarios/DP-R09-S01.yaml` | `pnpm verify:scenario -- DP-R09-S01` |
| DP-R09-S02 | 会话恢复 | 部分歌曲不可用 | 7.6, 8.9 | Desktop | `tests/scenarios/DP-R09-S02.yaml` | `pnpm verify:scenario -- DP-R09-S02` |
| DP-R09-S03 | 会话恢复 | 文件关联覆盖普通恢复 | — | Gate | `tests/scenarios/DP-R09-S03.yaml` | `pnpm verify:scenario -- DP-R09-S03` |
| DP-R10-S01 | 媒体键与快捷键 | 使用媒体键 | 9.4, 11.8 | Desktop/React/Native | `tests/native/DP-R10-S01.md` | `pnpm verify:scenario -- DP-R10-S01` |
| DP-R10-S02 | 媒体键与快捷键 | 使用应用快捷键 | 9.4, 11.8 | Desktop/React/Native | `tests/native/DP-R10-S02.md` | `pnpm verify:scenario -- DP-R10-S02` |
| DP-R11-S01 | 后台与托盘控制 | 关闭窗口退出 | 9.3, 9.6 | Desktop/Native | `tests/native/DP-R11-S01.md` | `pnpm verify:scenario -- DP-R11-S01` |
| DP-R11-S02 | 后台与托盘控制 | 关闭窗口后台运行 | 9.3, 9.6 | Desktop/Native | `tests/native/DP-R11-S02.md` | `pnpm verify:scenario -- DP-R11-S02` |
| DP-R11-S03 | 后台与托盘控制 | 托盘控制 | — | Gate | `tests/scenarios/DP-R11-S03.yaml` | `pnpm verify:scenario -- DP-R11-S03` |
| DP-R12-S01 | 三天播放历史与回退 | 列表循环上一首 | — | Gate | `tests/scenarios/DP-R12-S01.yaml` | `pnpm verify:scenario -- DP-R12-S01` |
| DP-R12-S02 | 三天播放历史与回退 | 三天内回退 | — | Gate | `tests/scenarios/DP-R12-S02.yaml` | `pnpm verify:scenario -- DP-R12-S02` |
| DP-R12-S03 | 三天播放历史与回退 | 历史跨重启 | — | Gate | `tests/scenarios/DP-R12-S03.yaml` | `pnpm verify:scenario -- DP-R12-S03` |
| DP-R12-S04 | 三天播放历史与回退 | 过期或失效历史 | — | Gate | `tests/scenarios/DP-R12-S04.yaml` | `pnpm verify:scenario -- DP-R12-S04` |
| DP-R12-S05 | 三天播放历史与回退 | 删除回滚保留历史 | — | Gate | `tests/scenarios/DP-R12-S05.yaml` | `pnpm verify:scenario -- DP-R12-S05` |
| DP-R13-S01 | 非沉浸播放模式视觉语义 | 非沉浸模式切换为随机播放 | — | Gate | `tests/scenarios/DP-R13-S01.yaml` | `pnpm verify:scenario -- DP-R13-S01` |
| DP-R13-S02 | 非沉浸播放模式视觉语义 | 主题切换期间保持随机模式 | — | Gate | `tests/scenarios/DP-R13-S02.yaml` | `pnpm verify:scenario -- DP-R13-S02` |
| DP-R14-S01 | 播放位置的本机会话持久化 | 退出后恢复播放位置 | — | Gate | `tests/scenarios/DP-R14-S01.yaml` | `pnpm verify:scenario -- DP-R14-S01` |
| DP-R14-S02 | 播放位置的本机会话持久化 | 持久化位置超出有效范围 | — | Gate | `tests/scenarios/DP-R14-S02.yaml` | `pnpm verify:scenario -- DP-R14-S02` |
| DP-R15-S01 | 播放上下文的解析归属与顺序语义 | 从超过单页上限的资料库视图开始播放 | — | Core | `tests/scenarios/DP-R15-S01.yaml` | `pnpm verify:scenario -- DP-R15-S01` |
| DP-R15-S02 | 播放上下文的解析归属与顺序语义 | 从"最近"视图开始播放 | — | Gate | `tests/native/DP-R15-S02.md` | `pnpm verify:scenario -- DP-R15-S02` |
| DP-R15-S03 | 播放上下文的解析归属与顺序语义 | 从歌单开始播放 | — | Core | `tests/scenarios/DP-R15-S03.yaml` | `pnpm verify:scenario -- DP-R15-S03` |
| DP-R15-S04 | 播放上下文的解析归属与顺序语义 | 选中项已不在上下文中 | — | Core | `tests/scenarios/DP-R15-S04.yaml` | `pnpm verify:scenario -- DP-R15-S04` |
| DP-R16-S01 | 无有效当前歌曲时选择初始待播放项 | 重新打开已有资料库 | — | Gate | `tests/native/DP-R16-S01.md` | `pnpm verify:scenario -- DP-R16-S01` |
| DP-R16-S02 | 无有效当前歌曲时选择初始待播放项 | 上次播放歌曲被外部删除 | — | Gate | `tests/native/DP-R16-S02.md` | `pnpm verify:scenario -- DP-R16-S02` |
| DP-R16-S03 | 无有效当前歌曲时选择初始待播放项 | 资料库为空 | — | Gate | `tests/native/DP-R16-S03.md` | `pnpm verify:scenario -- DP-R16-S03` |
| DP-R16-S04 | 无有效当前歌曲时选择初始待播放项 | 有效播放会话优先恢复 | — | Gate | `tests/native/DP-R16-S04.md` | `pnpm verify:scenario -- DP-R16-S04` |
| DP-R17-S01 | 初始待播放项不得改变播放设置 | 兜底选择保持用户播放设置 | — | Gate | `tests/native/DP-R17-S01.md` | `pnpm verify:scenario -- DP-R17-S01` |

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
| LE-R01-S04 | 资料库视图 | 切换歌手或专辑目录 | 6.1, 10.5 | Core/React | `tests/scenarios/LE-R01-S04.yaml` | `pnpm verify:scenario -- LE-R01-S04` |
| LE-R02-S01 | 资料库搜索 | 搜索多个字段 | 3.6, 3.7, 6.2, 10.5 | Core/React/Perf | `tests/scenarios/LE-R02-S01.yaml` | `pnpm verify:scenario -- LE-R02-S01` |
| LE-R02-S02 | 资料库搜索 | 搜索聚合目录 | 3.6, 3.7, 6.2, 10.5 | Core/React/Perf | `tests/scenarios/LE-R02-S02.yaml` | `pnpm verify:scenario -- LE-R02-S02` |
| LE-R02-S03 | 资料库搜索 | 搜索词为空 | 3.6, 3.7, 6.2, 10.5 | Core/React/Perf | `tests/scenarios/LE-R02-S03.yaml` | `pnpm verify:scenario -- LE-R02-S03` |
| LE-R02-S04 | 资料库搜索 | 搜索无结果 | 3.6, 3.7, 6.2, 10.5 | Core/React/Perf | `tests/scenarios/LE-R02-S04.yaml` | `pnpm verify:scenario -- LE-R02-S04` |
| LE-R02-S05 | 资料库搜索 | 切换活动资料库后查询 | — | Gate | `tests/scenarios/LE-R02-S05.yaml` | `pnpm verify:scenario -- LE-R02-S05` |
| LE-R03-S01 | 全部歌曲排序 | 选择排序字段 | 3.8, 6.1, 10.5 | Core/React | `tests/scenarios/LE-R03-S01.yaml` | `pnpm verify:scenario -- LE-R03-S01` |
| LE-R03-S02 | 全部歌曲排序 | 按专辑排序 | 1.x, 2.x | React | `tests/scenarios/LE-R03-S02.yaml` | `pnpm verify:scenario -- LE-R03-S02` |
| LE-R03-S03 | 全部歌曲排序 | 既有歌手排序术语 | 2.x | React | `tests/scenarios/LE-R03-S03.yaml` | `pnpm verify:scenario -- LE-R03-S03` |
| LE-R03-S04 | 全部歌曲排序 | 排序值相同 | 3.8, 6.1, 10.5 | Core/React | `tests/scenarios/LE-R03-S04.yaml` | `pnpm verify:scenario -- LE-R03-S04` |
| LE-R03-S05 | 全部歌曲排序 | 非全部歌曲视图排序 | 3.8, 6.1, 10.5 | Core/React | `tests/scenarios/LE-R03-S05.yaml` | `pnpm verify:scenario -- LE-R03-S05` |
| LE-R03-S06 | 全部歌曲排序 | 重启后恢复全部歌曲排序 | 2.x | React | `tests/scenarios/LE-R03-S06.yaml` | `pnpm verify:scenario -- LE-R03-S06` |
| LE-R04-S01 | 歌曲收藏 | 收藏歌曲 | 6.3, 10.6 | Core/React | `tests/scenarios/LE-R04-S01.yaml` | `pnpm verify:scenario -- LE-R04-S01` |
| LE-R04-S02 | 歌曲收藏 | 取消收藏 | 6.3, 10.6 | Core/React | `tests/scenarios/LE-R04-S02.yaml` | `pnpm verify:scenario -- LE-R04-S02` |
| LE-R05-S01 | 收藏事实时间真实性 | 标记喜欢 | 13.9, 13.13 | Core/Native | `tests/native/LE-R05-S01.md` | `pnpm verify:scenario -- LE-R05-S01` |
| LE-R05-S02 | 收藏事实时间真实性 | 取消后再次收藏 | 13.9, 13.13 | Core/Native | `tests/native/LE-R05-S02.md` | `pnpm verify:scenario -- LE-R05-S02` |
| LE-R05-S03 | 收藏事实时间真实性 | 收藏时间按当前时区呈现 | 13.9, 13.13 | Core/Native | `tests/native/LE-R05-S03.md` | `pnpm verify:scenario -- LE-R05-S03` |
| LE-R06-S01 | 歌曲操作与详情 | 打开歌曲操作菜单 | 6.4, 8.11, 10.6, 10.8 | Core/Desktop/React | `tests/scenarios/LE-R06-S01.yaml` | `pnpm verify:scenario -- LE-R06-S01` |
| LE-R06-S02 | 歌曲操作与详情 | 打开多选操作菜单 | 6.4, 8.11, 10.6, 10.8 | Core/Desktop/React | `tests/scenarios/LE-R06-S02.yaml` | `pnpm verify:scenario -- LE-R06-S02` |
| LE-R06-S03 | 歌曲操作与详情 | 查看歌曲详情 | 6.4, 8.11, 10.6, 10.8 | Core/Desktop/React | `tests/scenarios/LE-R06-S03.yaml` | `pnpm verify:scenario -- LE-R06-S03` |
| LE-R06-S04 | 歌曲操作与详情 | 删除当前播放歌曲 | 6.4, 8.11, 10.6, 10.8, 9.5 | Core/Desktop/React | `tests/scenarios/LE-R06-S04.yaml` | `pnpm verify:scenario -- LE-R06-S04` |
| LE-R06-S05 | 歌曲操作与详情 | 打开本地目录 | 6.4, 8.11, 10.6, 10.8 | Core/Desktop/React | `tests/scenarios/LE-R06-S05.yaml` | `pnpm verify:scenario -- LE-R06-S05` |
| LE-R06-S06 | 歌曲操作与详情 | 删除歌曲 | 6.4, 8.11, 10.6, 10.8, 5.8, 13.3 | Core/Desktop/React | `tests/scenarios/LE-R06-S06.yaml` | `pnpm verify:scenario -- LE-R06-S06` |
| LE-R06-S07 | 歌曲操作与详情 | 应用不重启时完成回收站最终化 | 6.1, 10.5 | Core/React | `tests/scenarios/LE-R06-S07.yaml` | `pnpm verify:scenario -- LE-R06-S07` |
| LE-R06-S08 | 歌曲操作与详情 | 系统回收站暂时失败后重试 | 6.1, 10.5 | Core/React | `tests/scenarios/LE-R06-S08.yaml` | `pnpm verify:scenario -- LE-R06-S08` |
| LE-R06-S09 | 歌曲操作与详情 | 回收站结果无法证明 | 6.4, 8.11, 10.6, 10.8, 5.8, 13.3 | Core/Desktop/React | `tests/scenarios/LE-R06-S09.yaml` | `pnpm verify:scenario -- LE-R06-S09` |
| LE-R06-S10 | 歌曲操作与详情 | 回收站目标受路径边界保护 | 6.1, 10.5 | Core/React | `tests/scenarios/LE-R06-S10.yaml` | `pnpm verify:scenario -- LE-R06-S10` |
| LE-R07-S01 | 视图内歌曲多选 | 选择和取消选择歌曲 | 4.10, 10.3, 10.7 | Core/React | `tests/scenarios/LE-R07-S01.yaml` | `pnpm verify:scenario -- LE-R07-S01` |
| LE-R07-S02 | 视图内歌曲多选 | 聚合目录不显示多选 | 4.10, 10.3, 10.7 | Core/React | `tests/scenarios/LE-R07-S02.yaml` | `pnpm verify:scenario -- LE-R07-S02` |
| LE-R07-S03 | 视图内歌曲多选 | 聚合详情支持全选 | 4.10, 10.3, 10.7 | Core/React | `tests/scenarios/LE-R07-S03.yaml` | `pnpm verify:scenario -- LE-R07-S03` |
| LE-R07-S04 | 视图内歌曲多选 | 多选模式右键未选中歌曲 | 4.10, 10.3, 10.7 | Core/React | `tests/scenarios/LE-R07-S04.yaml` | `pnpm verify:scenario -- LE-R07-S04` |
| LE-R07-S05 | 视图内歌曲多选 | 右键已选中歌曲保留选集 | 6.1, 10.5 | Core/React | `tests/scenarios/LE-R07-S05.yaml` | `pnpm verify:scenario -- LE-R07-S05` |
| LE-R07-S06 | 视图内歌曲多选 | 全选当前已加载歌曲 | 6.1, 10.5 | Core/React | `tests/scenarios/LE-R07-S06.yaml` | `pnpm verify:scenario -- LE-R07-S06` |
| LE-R07-S07 | 视图内歌曲多选 | 跨分页保留选择 | 6.1, 10.5 | Core/React | `tests/scenarios/LE-R07-S07.yaml` | `pnpm verify:scenario -- LE-R07-S07` |
| LE-R07-S08 | 视图内歌曲多选 | 查询上下文改变 | 6.1, 10.5 | Core/React | `tests/scenarios/LE-R07-S08.yaml` | `pnpm verify:scenario -- LE-R07-S08` |
| LE-R07-S09 | 视图内歌曲多选 | 虚拟列表中的选择 | 6.1, 10.5 | Core/React | `tests/scenarios/LE-R07-S09.yaml` | `pnpm verify:scenario -- LE-R07-S09` |
| LE-R08-S01 | 批量歌曲操作与结果反馈 | 批量收藏 | 3.8, 10.6, 12.5 | Core/React/Perf | `tests/scenarios/LE-R08-S01.yaml` | `pnpm verify:scenario -- LE-R08-S01` |
| LE-R08-S02 | 批量歌曲操作与结果反馈 | 批量加入歌单 | 3.8, 10.6, 12.5 | Core/React/Perf | `tests/scenarios/LE-R08-S02.yaml` | `pnpm verify:scenario -- LE-R08-S02` |
| LE-R08-S03 | 批量歌曲操作与结果反馈 | 歌单视图批量移除 | 6.1, 10.5 | Core/React | `tests/scenarios/LE-R08-S03.yaml` | `pnpm verify:scenario -- LE-R08-S03` |
| LE-R08-S04 | 批量歌曲操作与结果反馈 | 批量播放队列操作 | 6.1, 10.5 | Core/React | `tests/scenarios/LE-R08-S04.yaml` | `pnpm verify:scenario -- LE-R08-S04` |
| LE-R08-S05 | 批量歌曲操作与结果反馈 | 批量删除并撤回 | 6.1, 10.5 | Core/React | `tests/scenarios/LE-R08-S05.yaml` | `pnpm verify:scenario -- LE-R08-S05` |
| LE-R08-S06 | 批量歌曲操作与结果反馈 | 批量删除遇到不可删除项 | 6.1, 10.5 | Core/React | `tests/scenarios/LE-R08-S06.yaml` | `pnpm verify:scenario -- LE-R08-S06` |
| LE-R08-S07 | 批量歌曲操作与结果反馈 | 只读根目录 | 6.1, 10.5 | Core/React | `tests/scenarios/LE-R08-S07.yaml` | `pnpm verify:scenario -- LE-R08-S07` |
| LE-R08-S08 | 批量歌曲操作与结果反馈 | 批量操作失败 | 6.1, 10.5 | Core/React | `tests/scenarios/LE-R08-S08.yaml` | `pnpm verify:scenario -- LE-R08-S08` |
| LE-R09-S01 | 资料库状态反馈 | 初次加载 | nav 1.1–2.5, 4.1–4.5 | Core/Desktop/React | `tests/scenarios/LE-R09-S01.yaml` | `pnpm verify:scenario -- LE-R09-S01` |
| LE-R09-S02 | 资料库状态反馈 | 资料库为空 | nav 2.1, 4.2 | Core/React | `tests/scenarios/LE-R09-S02.yaml` | `pnpm verify:scenario -- LE-R09-S02` |
| LE-R09-S03 | 资料库状态反馈 | 资料库不可用 | nav 2.1–2.3 | Core | `tests/scenarios/LE-R09-S03.yaml` | `pnpm verify:scenario -- LE-R09-S03` |
| LE-R09-S04 | 资料库状态反馈 | 加载或扫描失败 | nav 1.1, 2.3 | Core | `tests/scenarios/LE-R09-S04.yaml` | `pnpm verify:scenario -- LE-R09-S04` |
| LE-R10-S01 | 大曲库浏览 | 浏览大量歌曲 | nav 4.2–4.4 | React | `tests/scenarios/LE-R10-S01.yaml` | `pnpm verify:scenario -- LE-R10-S01` |
| LE-R10-S02 | 大曲库浏览 | 搜索或排序后保持定位 | nav 4.4 | React | `tests/scenarios/LE-R10-S02.yaml` | `pnpm verify:scenario -- LE-R10-S02` |
| LE-R11-S01 | 资料库视图计数 | 未打开过的视图也显示计数 | nav 1.1–2.5, 4.1–4.5 | Core/Desktop/React | `tests/native/LE-R11-S01.md` | `pnpm verify:scenario -- LE-R11-S01` |
| LE-R11-S02 | 资料库视图计数 | 计数不受分页影响 | nav 2.1, 4.2 | Core/React | `tests/native/LE-R11-S02.md` | `pnpm verify:scenario -- LE-R11-S02` |
| LE-R11-S03 | 资料库视图计数 | 计数与视图定义一致 | nav 2.1–2.3 | Core | `tests/native/LE-R11-S03.md` | `pnpm verify:scenario -- LE-R11-S03` |
| LE-R11-S04 | 资料库视图计数 | 最近添加上限 | nav 1.1, 2.3 | Core | `tests/scenarios/LE-R11-S04.yaml` | `pnpm verify:scenario -- LE-R11-S04` |
| LE-R12-S01 | 分页歌曲列表显示匹配总数 | 打开未加载完的全部歌曲 | 1.x, 2.x | React | `tests/scenarios/LE-R12-S01.yaml` | `pnpm verify:scenario -- LE-R12-S01` |
| LE-R12-S02 | 分页歌曲列表显示匹配总数 | 打开未加载完的喜欢的音乐 | 1.x, 2.x | React | `tests/scenarios/LE-R12-S02.yaml` | `pnpm verify:scenario -- LE-R12-S02` |
| LE-R12-S03 | 分页歌曲列表显示匹配总数 | 搜索结果跨越多个分页 | 1.x | Core | `tests/scenarios/LE-R12-S03.yaml` | `pnpm verify:scenario -- LE-R12-S03` |
| LE-R12-S04 | 分页歌曲列表显示匹配总数 | 加载后续页 | 1.x, 2.x | React | `tests/scenarios/LE-R12-S04.yaml` | `pnpm verify:scenario -- LE-R12-S04` |
| LE-R12-S05 | 分页歌曲列表显示匹配总数 | 非分页列表 | 2.x | React | `tests/scenarios/LE-R12-S05.yaml` | `pnpm verify:scenario -- LE-R12-S05` |
| LE-R13-S01 | 计数失效与刷新 | 在其他视图切换收藏后计数更新 | nav 4.2–4.4 | React | `tests/scenarios/LE-R13-S01.yaml` | `pnpm verify:scenario -- LE-R13-S01` |
| LE-R13-S02 | 计数失效与刷新 | 导入与删除后计数更新 | nav 4.4 | React | `tests/scenarios/LE-R13-S02.yaml` | `pnpm verify:scenario -- LE-R13-S02` |
| LE-R13-S03 | 计数失效与刷新 | 播放栏导入后所有已挂载视图刷新 | 6.1, 10.5 | Native/Gate | `tests/native/LE-R13-S03.md` | `pnpm verify:scenario -- LE-R13-S03` |
| LE-R13-S04 | 计数失效与刷新 | 刷新期间不显示空白 | nav 4.2–4.3 | React | `tests/scenarios/LE-R13-S04.yaml` | `pnpm verify:scenario -- LE-R13-S04` |
| LE-R13-S05 | 计数失效与刷新 | 资料库不可用时 | nav 4.2 | React | `tests/scenarios/LE-R13-S05.yaml` | `pnpm verify:scenario -- LE-R13-S05` |
| LE-R14-S01 | 首次导入后资料库与播放栏显示选中歌曲 | 导入第一首歌曲 | — | Gate | `tests/native/LE-R14-S01.md` | `pnpm verify:scenario -- LE-R14-S01` |
| LE-R14-S02 | 首次导入后资料库与播放栏显示选中歌曲 | 首次导入失败 | — | Gate | `tests/native/LE-R14-S02.md` | `pnpm verify:scenario -- LE-R14-S02` |
| LE-R14-S03 | 首次导入后资料库与播放栏显示选中歌曲 | 已有歌曲时继续导入 | — | Gate | `tests/native/LE-R14-S03.md` | `pnpm verify:scenario -- LE-R14-S03` |
| LE-R15-S01 | 常驻播放栏临时歌曲导入 | 导入进行中保护按钮 | 6.1, 10.5 | Native/Gate | `tests/native/LE-R15-S01.md` | `pnpm verify:scenario -- LE-R15-S01` |
| LE-R15-S02 | 常驻播放栏临时歌曲导入 | 导入完成刷新列表和计数 | 6.1, 10.5 | Native/Gate | `tests/native/LE-R15-S02.md` | `pnpm verify:scenario -- LE-R15-S02` |
| LE-R15-S03 | 常驻播放栏临时歌曲导入 | 导入未提交 | 6.1, 10.5 | Native/Gate | `tests/native/LE-R15-S03.md` | `pnpm verify:scenario -- LE-R15-S03` |
| LE-R16-S01 | 歌曲音质徽标 | 显示 SQ 徽标 | 6.1, 10.5 | Native/Gate | `tests/native/LE-R16-S01.md` | `pnpm verify:scenario -- LE-R16-S01` |
| LE-R16-S02 | 歌曲音质徽标 | 显示 HQ 徽标 | 6.1, 10.5 | Native/Gate | `tests/native/LE-R16-S02.md` | `pnpm verify:scenario -- LE-R16-S02` |
| LE-R16-S03 | 歌曲音质徽标 | 不显示徽标 | 6.1, 10.5 | Native/Gate | `tests/native/LE-R16-S03.md` | `pnpm verify:scenario -- LE-R16-S03` |
| LE-R16-S04 | 歌曲音质徽标 | 同时满足 SQ 与 HQ 条件 | 6.1, 10.5 | Native/Gate | `tests/native/LE-R16-S04.md` | `pnpm verify:scenario -- LE-R16-S04` |
| LE-R16-S05 | 歌曲音质徽标 | 徽标不影响行操作 | 6.1, 10.5 | Native/Gate | `tests/native/LE-R16-S05.md` | `pnpm verify:scenario -- LE-R16-S05` |
| LE-R16-S06 | 歌曲音质徽标 | 旧歌曲即时生效 | 6.1, 10.5 | Native/Gate | `tests/native/LE-R16-S06.md` | `pnpm verify:scenario -- LE-R16-S06` |
| LE-R17-S01 | 歌曲行提供一致的下一首播放与操作菜单入口 | 行内加号下一首播放 | 6.1, 10.5 | Native/Gate | `tests/native/LE-R17-S01.md` | `pnpm verify:scenario -- LE-R17-S01` |
| LE-R17-S02 | 歌曲行提供一致的下一首播放与操作菜单入口 | 普通模式右键歌曲行 | 6.1, 10.5 | Native/Gate | `tests/native/LE-R17-S02.md` | `pnpm verify:scenario -- LE-R17-S02` |
| LE-R17-S03 | 歌曲行提供一致的下一首播放与操作菜单入口 | 多选模式右键歌曲行 | 6.1, 10.5 | Native/Gate | `tests/native/LE-R17-S03.md` | `pnpm verify:scenario -- LE-R17-S03` |
| LE-R18-S01 | 歌单内搜索 | 在歌单内搜索 | 1.x–3.x | React | `tests/scenarios/LE-R18-S01.yaml` | `pnpm verify:scenario -- LE-R18-S01` |
| LE-R18-S02 | 歌单内搜索 | 清空歌单搜索 | 1.x–3.x | React | `tests/scenarios/LE-R18-S02.yaml` | `pnpm verify:scenario -- LE-R18-S02` |
| LE-R18-S03 | 歌单内搜索 | 歌单搜索无结果 | 1.x–3.x | React | `tests/scenarios/LE-R18-S03.yaml` | `pnpm verify:scenario -- LE-R18-S03` |
| LE-R18-S04 | 歌单内搜索 | 歌单搜索不影响其他歌单 | 1.x–3.x | React | `tests/scenarios/LE-R18-S04.yaml` | `pnpm verify:scenario -- LE-R18-S04` |
| LE-R19-S01 | 定位当前播放歌曲 | 定位到正在播放的歌曲 | 4.x | React | `tests/scenarios/LE-R19-S01.yaml` | `pnpm verify:scenario -- LE-R19-S01` |
| LE-R19-S02 | 定位当前播放歌曲 | 视图无正在播放歌曲 | 4.x | React | `tests/scenarios/LE-R19-S02.yaml` | `pnpm verify:scenario -- LE-R19-S02` |
| LE-R19-S03 | 定位当前播放歌曲 | 播放歌曲不属于当前视图 | 4.x | React | `tests/scenarios/LE-R19-S03.yaml` | `pnpm verify:scenario -- LE-R19-S03` |
| LE-R19-S04 | 定位当前播放歌曲 | 定位目标位于列表末尾 | 4.x | React | `tests/scenarios/LE-R19-S04.yaml` | `pnpm verify:scenario -- LE-R19-S04` |
| LE-R19-S05 | 定位当前播放歌曲 | 最近添加视图定位 | 4.x | Native | `tests/native/LE-R19-S05.md` | `pnpm verify:scenario -- LE-R19-S05` |

## local-library

| Scenario ID | Requirement | Scenario | 任务 | 测试层 | 测试/步骤 manifest | 实际验收命令 |
|---|---|---|---|---|---|---|
| LL-R01-S01 | 单一资料库根目录与本地持久化 | 选择根目录并建立资料库 | 3.1, 4.1, 10.3 | Core/Desktop/E2E | `tests/scenarios/LL-R01-S01.yaml` | `pnpm verify:scenario -- LL-R01-S01` |
| LL-R01-S02 | 单一资料库根目录与本地持久化 | 安全切换活动根目录 | 3.1, 4.1, 10.3 | Core/Desktop/E2E | `tests/scenarios/LL-R01-S02.yaml` | `pnpm verify:scenario -- LL-R01-S02` |
| LL-R01-S03 | 单一资料库根目录与本地持久化 | 切换根目录失败 | 3.1, 4.1, 10.3 | Core/Desktop/E2E | `tests/scenarios/LL-R01-S03.yaml` | `pnpm verify:scenario -- LL-R01-S03` |
| LL-R01-S04 | 单一资料库根目录与本地持久化 | 本机数据库丢失后重新选择同一目录 | 15.2, 15.3 | Core | `tests/scenarios/LL-R01-S04.yaml` | `pnpm verify:scenario -- LL-R01-S04` |
| LL-R02-S01 | 扫描、监听与手动重扫 | 首次扫描发现支持文件 | 4.7, 4.9, 4.10 | Core/Infrastructure | `tests/scenarios/LL-R02-S01.yaml` | `pnpm verify:scenario -- LL-R02-S01` |
| LL-R02-S02 | 扫描、监听与手动重扫 | 忽略不支持文件 | 4.7, 4.9, 4.10 | Core/Infrastructure | `tests/scenarios/LL-R02-S02.yaml` | `pnpm verify:scenario -- LL-R02-S02` |
| LL-R02-S03 | 扫描、监听与手动重扫 | 忽略控制面目录 | — | Gate | `tests/scenarios/LL-R02-S03.yaml` | `pnpm verify:scenario -- LL-R02-S03` |
| LL-R02-S04 | 扫描、监听与手动重扫 | 不兼容旧资料库布局 | — | Gate | `tests/scenarios/LL-R02-S04.yaml` | `pnpm verify:scenario -- LL-R02-S04` |
| LL-R02-S05 | 扫描、监听与手动重扫 | 监听外部新增和修改 | 4.7, 4.9, 4.10 | Core/Infrastructure | `tests/scenarios/LL-R02-S05.yaml` | `pnpm verify:scenario -- LL-R02-S05` |
| LL-R02-S06 | 扫描、监听与手动重扫 | 从设置页手动重新扫描 | 3.1, 4.1, 10.3 | Core/Desktop/E2E | `tests/scenarios/LL-R02-S06.yaml` | `pnpm verify:scenario -- LL-R02-S06` |
| LL-R02-S07 | 扫描、监听与手动重扫 | 没有活动资料库时不能重扫 | 3.1, 4.1, 10.3 | Core/Desktop/E2E | `tests/scenarios/LL-R02-S07.yaml` | `pnpm verify:scenario -- LL-R02-S07` |
| LL-R03-S01 | 一期格式与内容解析矩阵 | 解析内嵌数据 | 1.6, 4.3–4.5, 8.12 | Core/Desktop/Native | `tests/native/LL-R03-S01.md` | `pnpm verify:scenario -- LL-R03-S01` |
| LL-R03-S02 | 一期格式与内容解析矩阵 | 解析同名 LRC 侧车 | 1.6, 4.3–4.5, 8.12 | Core/Desktop/Native | `tests/native/LL-R03-S02.md` | `pnpm verify:scenario -- LL-R03-S02` |
| LL-R03-S03 | 一期格式与内容解析矩阵 | 文件损坏或标签异常 | 1.6, 4.3–4.5, 8.12 | Core/Desktop/Native | `tests/native/LL-R03-S03.md` | `pnpm verify:scenario -- LL-R03-S03` |
| LL-R03-S04 | 一期格式与内容解析矩阵 | m4a 内嵌标签解析 | 3.1, 4.1, 10.3 | Core/Desktop/E2E | `tests/scenarios/LL-R03-S04.yaml` | `pnpm verify:scenario -- LL-R03-S04` |
| LL-R03-S05 | 一期格式与内容解析矩阵 | wav 无标签兜底 | 3.1, 4.1, 10.3 | Core/Desktop/E2E | `tests/scenarios/LL-R03-S05.yaml` | `pnpm verify:scenario -- LL-R03-S05` |
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
| LL-R07-S03 | 资料库不可用与删除恢复 | 删除歌曲后可重新导入 | 5.7–5.10, 8.11, 10.3, 10.8 | Core/Desktop/React/Native | `tests/native/LL-R07-S03.md` | `pnpm verify:scenario -- LL-R07-S03` |
| LL-R07-S04 | 资料库不可用与删除恢复 | 文件恢复 | 5.7–5.10, 8.11, 10.3, 10.8 | Core/Desktop/React/Native | `tests/native/LL-R07-S04.md` | `pnpm verify:scenario -- LL-R07-S04` |
| LL-R08-S01 | 跨平台路径、隐私与性能约束 | Unicode 和平台路径 | 4.2, 7.7, 12.5–12.7, 13.4 | Security/Perf/Native | `tests/native/LL-R08-S01.md` | `pnpm verify:scenario -- LL-R08-S01` |
| LL-R08-S02 | 跨平台路径、隐私与性能约束 | 扫描期间继续使用界面 | 4.2, 7.7, 12.5–12.7, 13.4 | Security/Perf/Native | `tests/native/LL-R08-S02.md` | `pnpm verify:scenario -- LL-R08-S02` |
| LL-R08-S03 | 跨平台路径、隐私与性能约束 | 不泄露本机路径 | 4.2, 7.7, 12.5–12.7, 13.4, 1.8, 7.8 | Security/Perf/Native | `tests/native/LL-R08-S03.md` | `pnpm verify:scenario -- LL-R08-S03` |
| LL-R09-S01 | 导入事实时间真实性与时区呈现 | 真实导入时间 | 5.5, 13.9, 13.13 | Core/Native | `tests/scenarios/LL-R09-S01.yaml` | `pnpm verify:scenario -- LL-R09-S01` |
| LL-R09-S02 | 导入事实时间真实性与时区呈现 | 重复或重扫不改写时间 | 5.5, 13.9, 13.13 | Core/Native | `tests/scenarios/LL-R09-S02.yaml` | `pnpm verify:scenario -- LL-R09-S02` |
| LL-R09-S03 | 导入事实时间真实性与时区呈现 | 当前时区转换 | 5.5, 13.9, 13.13 | Core/Native | `tests/scenarios/LL-R09-S03.yaml` | `pnpm verify:scenario -- LL-R09-S03` |

## phase-one-acceptance

| Scenario ID | Requirement | Scenario | 任务 | 测试层 | 测试/步骤 manifest | 实际验收命令 |
|---|---|---|---|---|---|---|
| PHA-R01-S01 | 一期日常流程验收基线 | 歌单与队列日常流程 | — | Gate | `tests/scenarios/PHA-R01-S01.yaml` | `pnpm verify:scenario -- PHA-R01-S01` |
| PHA-R01-S02 | 一期日常流程验收基线 | 回归质量门 | — | Gate | `tests/scenarios/PHA-R01-S02.yaml` | `pnpm verify:scenario -- PHA-R01-S02` |

## playlist-management

| Scenario ID | Requirement | Scenario | 任务 | 测试层 | 测试/步骤 manifest | 实际验收命令 |
|---|---|---|---|---|---|---|
| PM-R01-S01 | 歌单 CRUD | 创建歌单 | 6.5, 10.9 | Core/React | `tests/scenarios/PM-R01-S01.yaml` | `pnpm verify:scenario -- PM-R01-S01` |
| PM-R01-S02 | 歌单 CRUD | 名称无效 | 6.5, 10.9 | Core/React | `tests/scenarios/PM-R01-S02.yaml` | `pnpm verify:scenario -- PM-R01-S02` |
| PM-R01-S03 | 歌单 CRUD | 创建提交失败 | — | Gate | `tests/scenarios/PM-R01-S03.yaml` | `pnpm verify:scenario -- PM-R01-S03` |
| PM-R01-S04 | 歌单 CRUD | 重命名歌单 | 6.5, 10.9 | Core/React | `tests/scenarios/PM-R01-S04.yaml` | `pnpm verify:scenario -- PM-R01-S04` |
| PM-R01-S05 | 歌单 CRUD | 删除歌单 | 6.5, 10.9 | Core/React | `tests/scenarios/PM-R01-S05.yaml` | `pnpm verify:scenario -- PM-R01-S05` |
| PM-R02-S01 | 歌单查看与成员顺序 | 打开歌单 | 3.2, 6.1, 6.6, 10.9 | Core/React | `tests/scenarios/PM-R02-S01.yaml` | `pnpm verify:scenario -- PM-R02-S01` |
| PM-R02-S02 | 歌单查看与成员顺序 | 追加后查看 | 3.2, 6.1, 6.6, 10.9 | Core/React | `tests/scenarios/PM-R02-S02.yaml` | `pnpm verify:scenario -- PM-R02-S02` |
| PM-R03-S01 | 歌单成员添加 | 添加单首歌曲 | 6.6, 10.9 | Core/React | `tests/scenarios/PM-R03-S01.yaml` | `pnpm verify:scenario -- PM-R03-S01` |
| PM-R03-S02 | 歌单成员添加 | 添加到多个歌单 | 6.6, 10.9 | Core/React | `tests/scenarios/PM-R03-S02.yaml` | `pnpm verify:scenario -- PM-R03-S02` |
| PM-R03-S03 | 歌单成员添加 | 批量添加到多个歌单 | 6.6, 10.9 | Core/React | `tests/scenarios/PM-R03-S03.yaml` | `pnpm verify:scenario -- PM-R03-S03` |
| PM-R03-S04 | 歌单成员添加 | 在歌单视图中从单曲菜单添加 | 6.5, 10.9 | Core/React | `tests/scenarios/PM-R03-S04.yaml` | `pnpm verify:scenario -- PM-R03-S04` |
| PM-R03-S05 | 歌单成员添加 | 只读歌单视图禁用添加入口 | 6.5, 10.9 | Core/React | `tests/scenarios/PM-R03-S05.yaml` | `pnpm verify:scenario -- PM-R03-S05` |
| PM-R03-S06 | 歌单成员添加 | 重复添加 | 6.6, 10.9 | Core/React | `tests/scenarios/PM-R03-S06.yaml` | `pnpm verify:scenario -- PM-R03-S06` |
| PM-R03-S07 | 歌单成员添加 | 批量添加部分失败 | 6.5, 10.9 | Core/React | `tests/scenarios/PM-R03-S07.yaml` | `pnpm verify:scenario -- PM-R03-S07` |
| PM-R04-S01 | 歌单成员移除 | 移除成员 | 6.6, 10.9 | Core/React | `tests/scenarios/PM-R04-S01.yaml` | `pnpm verify:scenario -- PM-R04-S01` |
| PM-R04-S02 | 歌单成员移除 | 批量移除成员 | 6.5, 10.9 | Core/React | `tests/scenarios/PM-R04-S02.yaml` | `pnpm verify:scenario -- PM-R04-S02` |
| PM-R04-S03 | 歌单成员移除 | 批量移除部分失败 | 6.5, 10.9 | Core/React | `tests/scenarios/PM-R04-S03.yaml` | `pnpm verify:scenario -- PM-R04-S03` |
| PM-R05-S01 | 失效歌曲成员 | 文件暂时不可用 | 5.9, 6.7, 10.9 | Core/React | `tests/scenarios/PM-R05-S01.yaml` | `pnpm verify:scenario -- PM-R05-S01` |
| PM-R05-S02 | 失效歌曲成员 | 文件已删除 | 5.9, 6.7, 10.9 | Core/React | `tests/scenarios/PM-R05-S02.yaml` | `pnpm verify:scenario -- PM-R05-S02` |
| PM-R05-S03 | 失效歌曲成员 | 用户在 Echo 中删除歌曲 | 5.9, 6.7, 10.9 | Core/React | `tests/scenarios/PM-R05-S03.yaml` | `pnpm verify:scenario -- PM-R05-S03` |
| PM-R05-S04 | 失效歌曲成员 | 删除后仍在撤销窗口内 | 5.9, 6.7, 10.9 | Core/React | `tests/scenarios/PM-R05-S04.yaml` | `pnpm verify:scenario -- PM-R05-S04` |
| PM-R05-S05 | 失效歌曲成员 | 删除撤销 | 6.5, 10.9 | Core/React | `tests/scenarios/PM-R05-S05.yaml` | `pnpm verify:scenario -- PM-R05-S05` |
| PM-R05-S06 | 失效歌曲成员 | 失效歌曲恢复 | 5.9, 6.7, 10.9 | Core/React | `tests/scenarios/PM-R05-S06.yaml` | `pnpm verify:scenario -- PM-R05-S06` |
| PM-R06-S01 | 歌单异步操作反馈 | 成员移除失败 | — | Gate | `tests/scenarios/PM-R06-S01.yaml` | `pnpm verify:scenario -- PM-R06-S01` |
| PM-R06-S02 | 歌单异步操作反馈 | 入队失败 | — | Gate | `tests/scenarios/PM-R06-S02.yaml` | `pnpm verify:scenario -- PM-R06-S02` |
| PM-R06-S03 | 歌单异步操作反馈 | 批量操作结果 | 6.5, 10.9 | Core/React | `tests/scenarios/PM-R06-S03.yaml` | `pnpm verify:scenario -- PM-R06-S03` |
| PM-R07-S01 | 歌单选择器内新建歌单 | 从选择器创建并添加歌曲 | — | Gate | `tests/scenarios/PM-R07-S01.yaml` | `pnpm verify:scenario -- PM-R07-S01` |
| PM-R07-S02 | 歌单选择器内新建歌单 | 从选择器创建失败 | — | Gate | `tests/scenarios/PM-R07-S02.yaml` | `pnpm verify:scenario -- PM-R07-S02` |
| PM-R08-S01 | 歌单与成员事实时间真实性 | 创建歌单时记录时间 | 6.5, 6.6, 13.9, 13.13 | Core/Native | `tests/scenarios/PM-R08-S01.yaml` | `pnpm verify:scenario -- PM-R08-S01` |
| PM-R08-S02 | 歌单与成员事实时间真实性 | 加入歌单时记录时间 | 6.5, 6.6, 13.9, 13.13 | Core/Native | `tests/scenarios/PM-R08-S02.yaml` | `pnpm verify:scenario -- PM-R08-S02` |
| PM-R08-S03 | 歌单与成员事实时间真实性 | 重复加入不重写时间 | 6.5, 6.6, 13.9, 13.13 | Core/Native | `tests/scenarios/PM-R08-S03.yaml` | `pnpm verify:scenario -- PM-R08-S03` |
| PM-R08-S04 | 歌单与成员事实时间真实性 | 歌单时间按当前时区呈现 | 6.5, 6.6, 13.9, 13.13 | Core/Native | `tests/scenarios/PM-R08-S04.yaml` | `pnpm verify:scenario -- PM-R08-S04` |
| PM-R09-S01 | 歌单详情搜索 | 歌单详情提供搜索控件 | 1.x, 2.x, 3.x | React | `tests/scenarios/PM-R09-S01.yaml` | `pnpm verify:scenario -- PM-R09-S01` |
| PM-R09-S02 | 歌单详情搜索 | 歌单详情清空搜索 | 1.x, 2.x, 3.x | React | `tests/scenarios/PM-R09-S02.yaml` | `pnpm verify:scenario -- PM-R09-S02` |
| PM-R10-S01 | 歌单内定位当前播放歌曲 | 定位歌单中正在播放的歌曲 | 4.x | React | `tests/scenarios/PM-R10-S01.yaml` | `pnpm verify:scenario -- PM-R10-S01` |
| PM-R10-S02 | 歌单内定位当前播放歌曲 | 歌单无正在播放歌曲 | 4.x | React | `tests/scenarios/PM-R10-S02.yaml` | `pnpm verify:scenario -- PM-R10-S02` |

## portable-library-layout

| Scenario ID | Requirement | Scenario | 任务 | 测试层 | 测试/步骤 manifest | 实际验收命令 |
|---|---|---|---|---|---|---|
| PLL-R01-S01 | 受管理资料库目录布局 | 新资料库导入一首歌曲 | — | Gate | `tests/scenarios/PLL-R01-S01.yaml` | `pnpm verify:scenario -- PLL-R01-S01` |
| PLL-R01-S02 | 受管理资料库目录布局 | 控制面目录不可写 | — | Gate | `tests/scenarios/PLL-R01-S02.yaml` | `pnpm verify:scenario -- PLL-R01-S02` |
| PLL-R01-S03 | 受管理资料库目录布局 | 每种对象种类都有对应目录 | 15.1 | Gate | `tests/scenarios/PLL-R01-S03.yaml` | `pnpm verify:scenario -- PLL-R01-S03` |
| PLL-R02-S01 | 可携带对象资料 | 检查资料库控制面 | — | Gate | `tests/scenarios/PLL-R02-S01.yaml` | `pnpm verify:scenario -- PLL-R02-S01` |
| PLL-R02-S02 | 可携带对象资料 | 路径数据被序列化 | — | Gate | `tests/scenarios/PLL-R02-S02.yaml` | `pnpm verify:scenario -- PLL-R02-S02` |
| PLL-R02-S03 | 可携带对象资料 | 同步忽略临时目录 | — | Gate | `tests/scenarios/PLL-R02-S03.yaml` | `pnpm verify:scenario -- PLL-R02-S03` |
| PLL-R02-S04 | 可携带对象资料 | 歌单与成员的变更被材料化 | 15.4 | Gate | `tests/scenarios/PLL-R02-S04.yaml` | `pnpm verify:scenario -- PLL-R02-S04` |
| PLL-R02-S05 | 可携带对象资料 | 播放统计随资料库移动 | 15.4 | Gate | `tests/scenarios/PLL-R02-S05.yaml` | `pnpm verify:scenario -- PLL-R02-S05` |
| PLL-R02-S06 | 可携带对象资料 | 取消收藏不留下悬空记录 | 15.4 | Gate | `tests/scenarios/PLL-R02-S06.yaml` | `pnpm verify:scenario -- PLL-R02-S06` |
| PLL-R03-S01 | 歌曲入库时刻随资料库对象资料携带 | 扫描发现新歌曲写入入库时刻 | — | Gate | `tests/scenarios/PLL-R03-S01.yaml` | `pnpm verify:scenario -- PLL-R03-S01` |
| PLL-R03-S02 | 歌曲入库时刻随资料库对象资料携带 | 手动导入歌曲写入入库时刻 | — | Gate | `tests/scenarios/PLL-R03-S02.yaml` | `pnpm verify:scenario -- PLL-R03-S02` |
| PLL-R03-S03 | 歌曲入库时刻随资料库对象资料携带 | 入库时刻不被后续变更改写 | — | Gate | `tests/scenarios/PLL-R03-S03.yaml` | `pnpm verify:scenario -- PLL-R03-S03` |
| PLL-R04-S01 | 从资料库恢复或接续时还原歌曲入库时刻 | 本机数据库被清除后从资料库恢复保留最近添加顺序 | — | Gate | `tests/scenarios/PLL-R04-S01.yaml` | `pnpm verify:scenario -- PLL-R04-S01` |
| PLL-R04-S02 | 从资料库恢复或接续时还原歌曲入库时刻 | 新设备恢复时入库时刻随歌曲按原 UUID 还原 | — | Gate | `tests/scenarios/PLL-R04-S02.yaml` | `pnpm verify:scenario -- PLL-R04-S02` |
| PLL-R04-S03 | 从资料库恢复或接续时还原歌曲入库时刻 | 打开已有控制面的资料库目录时接续还原入库时刻 | — | Gate | `tests/scenarios/PLL-R04-S03.yaml` | `pnpm verify:scenario -- PLL-R04-S03` |
| PLL-R04-S04 | 从资料库恢复或接续时还原歌曲入库时刻 | 历史对象记录缺入库时刻仍可兼容恢复 | — | Gate | `tests/native/PLL-R04-S04.md` | `pnpm verify:scenario -- PLL-R04-S04` |
| PLL-R05-S01 | 新设备资料库重建 | 全新设备恢复完整资料 | 15.3 | Gate | `tests/scenarios/PLL-R05-S01.yaml` | `pnpm verify:scenario -- PLL-R05-S01` |
| PLL-R05-S02 | 新设备资料库重建 | 同步资料先于媒体到达 | 15.3 | Gate | `tests/scenarios/PLL-R05-S02.yaml` | `pnpm verify:scenario -- PLL-R05-S02` |
| PLL-R05-S03 | 新设备资料库重建 | 本机数据库丢失后重开同一资料库 | 15.3 | Gate | `tests/scenarios/PLL-R05-S03.yaml` | `pnpm verify:scenario -- PLL-R05-S03` |
| PLL-R05-S04 | 新设备资料库重建 | 重复打开结果稳定 | 15.3 | Gate | `tests/scenarios/PLL-R05-S04.yaml` | `pnpm verify:scenario -- PLL-R05-S04` |
| PLL-R06-S01 | 控制面存在性与自愈 | 首次启用即建立 manifest | 15.2 | Gate | `tests/scenarios/PLL-R06-S01.yaml` | `pnpm verify:scenario -- PLL-R06-S01` |
| PLL-R06-S02 | 控制面存在性与自愈 | manifest 缺失时自愈 | 15.2 | Gate | `tests/scenarios/PLL-R06-S02.yaml` | `pnpm verify:scenario -- PLL-R06-S02` |
| PLL-R06-S03 | 控制面存在性与自愈 | 控制面不可写时不破坏既有内容 | 15.2 | Gate | `tests/scenarios/PLL-R06-S03.yaml` | `pnpm verify:scenario -- PLL-R06-S03` |
| PLL-R07-S01 | 对象资料接续与对账 | 打开资料库先接续再扫描 | 15.3 | Gate | `tests/scenarios/PLL-R07-S01.yaml` | `pnpm verify:scenario -- PLL-R07-S01` |
| PLL-R07-S02 | 对象资料接续与对账 | 对象资料与媒体不一致 | 15.3 | Gate | `tests/scenarios/PLL-R07-S02.yaml` | `pnpm verify:scenario -- PLL-R07-S02` |
| PLL-R07-S03 | 对象资料接续与对账 | 墓碑优先于陈旧记录 | 15.3 | Gate | `tests/scenarios/PLL-R07-S03.yaml` | `pnpm verify:scenario -- PLL-R07-S03` |
| PLL-R07-S04 | 对象资料接续与对账 | 重复接续结果稳定 | 15.3 | Gate | `tests/scenarios/PLL-R07-S04.yaml` | `pnpm verify:scenario -- PLL-R07-S04` |
| PLL-R07-S05 | 对象资料接续与对账 | 较新的本机身份不被旧记录覆盖 | 15.3 | Gate | `tests/scenarios/PLL-R07-S05.yaml` | `pnpm verify:scenario -- PLL-R07-S05` |

## safe-file-ingestion

| Scenario ID | Requirement | Scenario | 任务 | 测试层 | 测试/步骤 manifest | 实际验收命令 |
|---|---|---|---|---|---|---|
| SFI-R01-S01 | 多选导入与默认目标命名 | 多选文件成功导入 | 5.1, 5.2, 10.10 | Core/React/E2E | `tests/scenarios/SFI-R01-S01.yaml` | `pnpm verify:scenario -- SFI-R01-S01` |
| SFI-R01-S02 | 多选导入与默认目标命名 | 标签缺失和非法字符 | 5.1, 5.2, 10.10 | Core/React/E2E | `tests/scenarios/SFI-R01-S02.yaml` | `pnpm verify:scenario -- SFI-R01-S02` |
| SFI-R01-S03 | 多选导入与默认目标命名 | 无标签 wav 导入 | 5.1, 5.2, 10.10 | Core/React/E2E | `tests/scenarios/SFI-R01-S03.yaml` | `pnpm verify:scenario -- SFI-R01-S03` |
| SFI-R02-S01 | 导入选择过滤器与支持矩阵一致 | 过滤器列出且仅列出支持格式 | 5.4, 10.10 | Core/React | `tests/scenarios/SFI-R02-S01.yaml` | `pnpm verify:scenario -- SFI-R02-S01` |
| SFI-R03-S01 | 同名歌词侧车导入 | 音频与 LRC 一起导入 | 5.2, 5.6 | Core/Infrastructure | `tests/scenarios/SFI-R03-S01.yaml` | `pnpm verify:scenario -- SFI-R03-S01` |
| SFI-R03-S02 | 同名歌词侧车导入 | LRC 不可读 | 5.2, 5.6 | Core/Infrastructure | `tests/scenarios/SFI-R03-S02.yaml` | `pnpm verify:scenario -- SFI-R03-S02` |
| SFI-R04-S01 | BLAKE3 去重与重名编号 | 内容重复 | 5.3–5.5, 13.3 | Core/Fault injection | `tests/scenarios/SFI-R04-S01.yaml` | `pnpm verify:scenario -- SFI-R04-S01` |
| SFI-R04-S02 | BLAKE3 去重与重名编号 | 已删除歌曲允许重新导入 | 5.3–5.5, 13.3 | Core/Fault injection | `tests/scenarios/SFI-R04-S02.yaml` | `pnpm verify:scenario -- SFI-R04-S02` |
| SFI-R04-S03 | BLAKE3 去重与重名编号 | 不可用记录不占用旧目标路径 | 5.3–5.5, 13.3 | Core/Fault injection | `tests/scenarios/SFI-R04-S03.yaml` | `pnpm verify:scenario -- SFI-R04-S03` |
| SFI-R04-S04 | BLAKE3 去重与重名编号 | 同名不同内容 | 5.3–5.5, 13.3 | Core/Fault injection | `tests/scenarios/SFI-R04-S04.yaml` | `pnpm verify:scenario -- SFI-R04-S04` |
| SFI-R05-S01 | 暂存、校验、原子移动与操作日志 | 校验失败不落库 | 5.1, 10.3, 10.10 | Core/React | `tests/scenarios/SFI-R05-S01.yaml` | `pnpm verify:scenario -- SFI-R05-S01` |
| SFI-R05-S02 | 暂存、校验、原子移动与操作日志 | 导入成功原子可见 | 5.1, 10.3, 10.10 | Core/React | `tests/scenarios/SFI-R05-S02.yaml` | `pnpm verify:scenario -- SFI-R05-S02` |
| SFI-R05-S03 | 暂存、校验、原子移动与操作日志 | 崩溃后恢复 | 5.3–5.5, 13.3 | Core/Fault injection | `tests/scenarios/SFI-R05-S03.yaml` | `pnpm verify:scenario -- SFI-R05-S03` |
| SFI-R05-S04 | 暂存、校验、原子移动与操作日志 | 音频发布后侧车失败 | 5.3–5.5, 13.3 | Core/Fault injection | `tests/scenarios/SFI-R05-S04.yaml` | `pnpm verify:scenario -- SFI-R05-S04` |
| SFI-R05-S05 | 暂存、校验、原子移动与操作日志 | 发布后 watcher 抢先观察 | 5.3–5.5, 13.3, 4.9, 5.5 | Core/Fault injection | `tests/scenarios/SFI-R05-S05.yaml` | `pnpm verify:scenario -- SFI-R05-S05` |
| SFI-R06-S01 | 逐文件结果与资料库不可用反馈 | 混合结果 | 4.2, 7.5, 9.1, 9.2 | Security/Native | `tests/native/SFI-R06-S01.md` | `pnpm verify:scenario -- SFI-R06-S01` |
| SFI-R06-S02 | 逐文件结果与资料库不可用反馈 | 导入时根目录断开 | 4.2, 7.5, 9.1, 9.2 | Security/Native | `tests/native/SFI-R06-S02.md` | `pnpm verify:scenario -- SFI-R06-S02` |
| SFI-R07-S01 | 源文件、系统关联与安全边界 | 外部文件直接打开 | 9.1, 9.2 | Desktop/Native | `tests/native/SFI-R07-S01.md` | `pnpm verify:scenario -- SFI-R07-S01` |
| SFI-R07-S02 | 源文件、系统关联与安全边界 | 活动资料库内文件直接打开 | 9.1, 9.2 | Desktop/Native | `tests/native/SFI-R07-S02.md` | `pnpm verify:scenario -- SFI-R07-S02` |
| SFI-R07-S03 | 源文件、系统关联与安全边界 | 非活动旧资料库文件直接打开 | 4.2, 7.5, 9.1, 9.2, 8.9 | Security/Native | `tests/native/SFI-R07-S03.md` | `pnpm verify:scenario -- SFI-R07-S03` |
| SFI-R07-S04 | 源文件、系统关联与安全边界 | 源文件保持不变 | 4.2, 7.5, 9.2, 12.7, 5.3 | Security/Native | `tests/native/SFI-R07-S04.md` | `pnpm verify:scenario -- SFI-R07-S04` |
| SFI-R07-S05 | 源文件、系统关联与安全边界 | 暂存目录名称与用户内容冲突 | 4.2, 7.5, 9.2, 12.7, 5.3, 5.7 | Security/Native | `tests/native/SFI-R07-S05.md` | `pnpm verify:scenario -- SFI-R07-S05` |
| SFI-R08-S01 | 单实例唤醒与重复打开 | 已运行实例接收文件关联 | 9.1, 9.2 | Desktop/Native | `tests/scenarios/SFI-R08-S01.yaml` | `pnpm verify:scenario -- SFI-R08-S01` |
| SFI-R08-S02 | 单实例唤醒与重复打开 | 冷启动文件关联 | 9.1, 9.2 | Desktop/Native | `tests/scenarios/SFI-R08-S02.yaml` | `pnpm verify:scenario -- SFI-R08-S02` |
| SFI-R09-S01 | 操作系统文件打开载荷必须先归一化为本地路径 | 带百分号编码的 file URL 被归一化 | 5.5, 5.10, 13.3, 13.4 | Fault injection/Native | `tests/native/SFI-R09-S01.md` | `pnpm verify:scenario -- SFI-R09-S01` |
| SFI-R09-S02 | 操作系统文件打开载荷必须先归一化为本地路径 | 非文件 scheme 被丢弃 | 5.5, 5.10, 13.3, 13.4 | Fault injection/Native | `tests/native/SFI-R09-S02.md` | `pnpm verify:scenario -- SFI-R09-S02` |
| SFI-R09-S03 | 操作系统文件打开载荷必须先归一化为本地路径 | 归一化契约由类型强制 | — | Gate | `tests/scenarios/SFI-R09-S03.yaml` | `pnpm verify:scenario -- SFI-R09-S03` |
| SFI-R09-S04 | 操作系统文件打开载荷必须先归一化为本地路径 | 前端不得二次解码 | — | Gate | `tests/scenarios/SFI-R09-S04.yaml` | `pnpm verify:scenario -- SFI-R09-S04` |
| SFI-R09-S05 | 操作系统文件打开载荷必须先归一化为本地路径 | 纵深防御不得被放宽 | — | Gate | `tests/scenarios/SFI-R09-S05.yaml` | `pnpm verify:scenario -- SFI-R09-S05` |
| SFI-R10-S01 | 跨平台路径与恢复后的幂等性 | 跨平台安全命名 | 2.1, 2.2, 13.9, 13.13 | Core/Fault injection | `tests/scenarios/SFI-R10-S01.yaml` | `pnpm verify:scenario -- SFI-R10-S01` |
| SFI-R10-S02 | 跨平台路径与恢复后的幂等性 | 重试导入幂等 | 2.1, 2.2, 13.9, 13.13 | Core/Fault injection | `tests/scenarios/SFI-R10-S02.yaml` | `pnpm verify:scenario -- SFI-R10-S02` |
| SFI-R11-S01 | 受控并发批量导入与非阻塞反馈 | 大批量并发导入 | 2.1, 2.2, 13.9, 13.13 | Core/Fault injection | `tests/scenarios/SFI-R11-S01.yaml` | `pnpm verify:scenario -- SFI-R11-S01` |
| SFI-R11-S02 | 受控并发批量导入与非阻塞反馈 | 无失败结果完成 | 2.1, 2.2, 13.9, 13.13 | Core/Fault injection | `tests/scenarios/SFI-R11-S02.yaml` | `pnpm verify:scenario -- SFI-R11-S02` |
| SFI-R11-S03 | 受控并发批量导入与非阻塞反馈 | 批次包含失败 | 2.1, 2.2, 13.9, 13.13 | Core/Fault injection | `tests/scenarios/SFI-R11-S03.yaml` | `pnpm verify:scenario -- SFI-R11-S03` |
| SFI-R11-S04 | 受控并发批量导入与非阻塞反馈 | 全部项目失败 | 2.1, 2.2, 13.9, 13.13 | Core/Fault injection | `tests/scenarios/SFI-R11-S04.yaml` | `pnpm verify:scenario -- SFI-R11-S04` |
| SFI-R11-S05 | 受控并发批量导入与非阻塞反馈 | 多线程重叠可验证 | 2.1, 2.2, 13.9, 13.13 | Core/Fault injection | `tests/scenarios/SFI-R11-S05.yaml` | `pnpm verify:scenario -- SFI-R11-S05` |

## sync-foundation

| Scenario ID | Requirement | Scenario | 任务 | 测试层 | 测试/步骤 manifest | 实际验收命令 |
|---|---|---|---|---|---|---|
| SYN-R01-S01 | 同步基础数据形状（schema 骨架） | 数据形状已就绪且离线边界不变 | 3.10, 3.13, 3.14, 13.8 | Core/Gate | `tests/scenarios/SYN-R01-S01.yaml` | `pnpm verify:scenario -- SYN-R01-S01` |
| SYN-R01-S02 | 同步基础数据形状（schema 骨架） | 对象级 revision 单调递增 | 3.11 | Core | `tests/scenarios/SYN-R01-S02.yaml` | `pnpm verify:scenario -- SYN-R01-S02` |
| SYN-R02-S01 | 本地变更预写 outbox | 导入与收藏变更是 outbox 条目 | 3.11 | Core | `tests/scenarios/SYN-R02-S01.yaml` | `pnpm verify:scenario -- SYN-R02-S01` |
| SYN-R02-S02 | 本地变更预写 outbox | 预写不产生可操作同步 | 3.14, 13.8 | Core/Gate | `tests/scenarios/SYN-R02-S02.yaml` | `pnpm verify:scenario -- SYN-R02-S02` |
| SYN-R03-S01 | 墓碑 | 删除产生墓碑 | 3.12 | Core | `tests/scenarios/SYN-R03-S01.yaml` | `pnpm verify:scenario -- SYN-R03-S01` |
| SYN-R04-S01 | 可同步载荷不含本机路径 | 本机绝对路径不进入可同步载荷 | 3.13 | Core | `tests/scenarios/SYN-R04-S01.yaml` | `pnpm verify:scenario -- SYN-R04-S01` |

## 发布审计

1. 运行 `pnpm verify:scenario -- --all`，保存逐场景结果与集合差异报告。
2. P0 文件安全、恢复、路径边界与回收站场景必须是自动故障注入，不得只用人工步骤。
3. 执行 PRD A1–A14、完整质量命令和三平台原生矩阵；任何缺失映射或 P0 失败阻断 0.1.0。
