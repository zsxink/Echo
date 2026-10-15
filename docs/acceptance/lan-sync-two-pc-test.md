# 两台电脑局域网同步冒烟验证

本次双机验证使用 0.3.0。已由 [GitHub Actions Release run 38702819063](https://github.com/zsxink/Echo/actions/runs/38702819063) 从提交 `6a1bbe9` 构建成功；三个平台 job 均通过，`tag` 留空，因此没有发布 GitHub Release。到该 run 的 Artifacts 下载 `echo-windows` 和 `echo-macos`；Windows 安装包位于 `nsis/`，Mac 主机 DMG 位于 `dmg/`。Windows 安装器 SHA256：`3aca5f6178a875db50103f034526a0adc169c6d1ec9677e4b240b4139de53045`。Mac DMG SHA256：`5917ef0f01d75dfedcb91b71ef2ddcc2989b9fd0f6d16849ab0a2683032680be`。

## 构建并安装 Windows 从机

1. 打开 GitHub 仓库 Actions，选择 `Release` workflow，点击 `Run workflow`，分支选择本次功能分支，`tag` 留空，然后启动构建。
2. 等待三个 `Build` job 成功。在该次运行的 Artifacts 中下载 `echo-windows`，解压后运行 `nsis/Echo_0.3.0_x64-setup.exe`。同时下载 `echo-macos`，取得 Mac 主机 DMG。
3. 安装包未签名；Windows SmartScreen 若拦截，先从 Actions 的 `SHA256SUMS-windows-x64` 核对 exe，再选择“仍要运行”。

## 建立连接

1. 两台电脑接入同一个可信的家庭/办公局域网；不要使用访客 Wi-Fi、VPN 或开启客户端隔离的热点。
2. 在 Mac 上安装 Actions 的 `Echo_0.3.0_aarch64.dmg`。首次打开未签名应用时，在 Finder 对 Echo 右键选择“打开”，再按 macOS 提示确认。
3. Mac 上先选择包含现有曲库的 Echo 音乐目录。在“设置 → 局域网资料库同步”选“主机”，填 Mac 的 Wi-Fi 局域网 IP（不要填 `127.0.0.1`），启动主机。macOS 防火墙询问时允许 Echo 接收传入连接。
4. 主机启动后生成邀请。它含短时配对凭据；通过 USB 或你信任的本地传递方式送到 Windows，不要贴到公开网站或 issue。
5. Windows 上先在 Echo 中选择一个**新建的空目录**作为音乐目录。不要选择现有音乐库、桌面、下载目录或磁盘根目录。
6. 在同一设置页选“加入设备”，粘贴邀请、输入设备名并发送请求。
7. 回到 Mac 刷新“待审批设备”并批准。Windows 随后点“检查并领取连接”。Windows Defender 防火墙若询问网络权限，仅对“专用网络”允许 Echo。
8. Windows 显示“已连接”后点“立即同步”，等待完成。首次同步会把主机快照和媒体资源下载到 Windows 的空资料库；曲目多或文件大时会花较长时间。

## 验证双向变化

1. 在 Windows 导入一首测试音频并点“立即同步”。
2. Mac 点“立即同步”，确认这首歌出现在曲库且能播放。
3. 在 Mac 收藏该曲目或创建/修改一个歌单并点“立即同步”；Windows 再点“立即同步”，确认变化到达。
4. 可在两台电脑分别断开 Wi-Fi、各做一项不同的元数据修改，再恢复网络并交替点“立即同步”，观察状态和最终内容。
5. 记录两端 Echo 版本、操作顺序、成功/失败提示，以及 `echo/` 和 `media/` 中结果。首轮只用可删除的测试资料库和测试曲目。

## 失败时先检查

- 两台机器的 IP 是否在同一子网；Mac 主机界面的连接地址是否仍是当前 IP。
- Mac 防火墙是否允许 Echo 入站、Windows 是否选择了专用网络；路由器是否启用了客户端隔离。
- 邀请是否已过 5 分钟或被重复使用；过期时在 Mac 重新生成邀请。
- Windows 目标必须是 Echo 已选中的新空库。Echo 自动创建的 `echo/manifest.json` 和 `echo/tmp` 所有权标记属于空库骨架；测试代码现在允许此骨架，同时拒绝媒体文件、其他文件和符号链接。
- 若请求被拒绝，先检查两端设置页显示的错误，再停止两端同步服务并重启应用；不要删除 SQLite、凭据库或 `echo/` 来尝试恢复。

这是两台真实电脑的冒烟验证步骤，不覆盖 OpenSpec 要求的完整故障矩阵、覆盖率门槛或跨平台发布验收。
