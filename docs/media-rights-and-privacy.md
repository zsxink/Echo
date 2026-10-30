# 用户媒体权利与隐私边界 / Media rights & privacy

> 本文档是 Echo 的用户媒体内容责任与隐私边界的事实来源（中英双语）。发布面向公众的版本前，请对照文末检查单逐项核对。

## 一、用户媒体内容责任（Media rights）

Echo 是**本地优先**的音乐播放器：它扫描、管理、播放**你自己的**本地音乐文件，并可在你自己的设备之间通过局域网同步曲库。在此过程中：

- **导入会复制你的音频文件**到资料库 `media/` 目录；**局域网同步会在你的配对设备之间复制音频文件、同名 `.lrc` 歌词文件与曲库元数据**。
- 你须**自行确保**拥有在你自己的设备间复制、同步这些内容的权利（例如：你拥有这些文件的版权，或复制/备份属于你所在司法辖区的私人使用例外）。
- **Echo 不提供、不下载任何音频内容**，没有流媒体曲库，也不内置联网获取歌词、封面或元数据的在线服务；歌词只来自音频内嵌标签、同名 `.lrc` 文件或你手动录入的覆盖层。
- **Echo 未取得、也不转授任何音乐、歌词或封面的版权授权**。Echo 提供的是整理与播放工具，内容责任由用户自负。

## 二、局域网同步的范围（LAN sync scope）

| 项目 | 说明 |
|---|---|
| 启用方式 | 默认关闭；须由用户在设置中显式开启（开启主机服务或将设备配对到主机） |
| 传输通道 | 仅限局域网内配对设备之间，HTTPS/TLS 加密，端口 `50826`；配对后设备身份以证书固定 |
| 同步对象 | 音频文件、同名 `.lrc` 歌词文件、曲库元数据（曲目/专辑/歌单/播放列表等变更记录） |
| 不同步 | SQLite 数据库文件（每台设备保留自己的投影）、诊断日志、应用配置 |
| 云端 | **无云端中继、无账号、无公共服务器**；流量不出局域网 |

同步的恢复与故障处置见 [`docs/lan-sync-recovery.md`](lan-sync-recovery.md)。

## 三、隐私边界（Privacy boundary）

当前版本的 Echo：

- **无账号体系**：没有注册、登录或用户身份系统。
- **无遥测、无崩溃上报**：不向任何服务器发送使用统计、崩溃转储或诊断信息。
- **无云端备份、无云端中继**：数据只存在于本机与你局域网内的配对设备上。
- **无联网内容抓取**：不在线获取歌词、封面或元数据。
- **诊断日志仅保存在本地设备**（按日期切分、保留最近 7 天），用户可随时删除；Echo 不会将其上传。

### 未来功能的前置义务（Future features）

若未来版本引入**任何使数据离开本机或局域网**的能力——账号、云端备份/中继、遥测、崩溃上报、远程（非局域网）同步、在线元数据服务等——**在发布前必须先更新本文档**，补充：收集哪些数据、处理目的、留存期限、用户权利（查看/导出/删除）与退出方式；未更新不得发布。

## 四、发布前检查单（Pre-release checklist）

面向公众的正式发布前逐项核对：

1. **律师复核**：由熟悉软件许可/版权的律师复核项目许可证（`LICENSE`，Echo 个人学习使用许可）与依赖分发方案（各平台 `vendor/libmpv/*/manifest.json`、`NOTICE.md` 与随包的 `THIRD-PARTY-LICENSES.txt`；Rust/npm 依赖清单 `LICENSES.md`）。**未完成不得发布。**
2. **许可文本随包**：确认三个平台的安装包内均可获取 `THIRD-PARTY-LICENSES.txt` 与对应平台 `NOTICE.md`（macOS `Contents/Resources/third-party/libmpv/`；Windows 安装根目录；Linux `/usr/share/doc/echo/`）。
3. **对应源码可获取**：确认各平台 NOTICE 指向的对应源码链接在发布时仍然可访问。
4. **文档一致**：本文档、双语 README 的「用户媒体权利与隐私」章节与实际行为一致；若本版本新增了数据外发能力，确认第三节的前置义务已履行。

---

# Media rights & privacy (English)

> This document is the source of truth for Echo's media-rights responsibilities and privacy boundary. Check the pre-release checklist below before publishing to the public.

## 1. Media rights

Echo is a **local-first** music player: it scans, manages, and plays **your own** local music files, and can sync your library between **your own** devices over a local network. In doing so:

- **Importing copies your audio files** into the library `media/` folder, and **LAN sync copies audio files, matching `.lrc` lyrics files, and library metadata between your paired devices**.
- **You are responsible** for ensuring you have the rights to copy and sync that content between your own devices (for example, you own the copyright, or private copying/backup falls under an exception in your jurisdiction).
- **Echo does not provide or download any audio content.** There is no streaming catalog and no online service that fetches lyrics, cover art, or metadata; lyrics come only from embedded audio tags, matching `.lrc` files, or user-entered overrides.
- **Echo obtains no music, lyrics, or cover-art licenses and grants none.** Echo is an organizer and player; responsibility for the content rests with the user.

## 2. LAN sync scope

| Item | Detail |
|---|---|
| Enablement | Off by default; the user must explicitly start hosting or pair a device in Settings |
| Channel | Between paired devices on the local network only, over HTTPS/TLS, port `50826`; device identity is certificate-pinned after pairing |
| What syncs | Audio files, matching `.lrc` lyrics files, and library metadata (track/album/playlist change records) |
| What does not sync | SQLite database files (each device keeps its own projection), diagnostic logs, app configuration |
| Cloud | **No cloud relay, no accounts, no public servers**; traffic never leaves the LAN |

Recovery and troubleshooting: see [`docs/lan-sync-recovery.md`](lan-sync-recovery.md).

## 3. Privacy boundary

The current version of Echo:

- **Has no accounts** — no sign-up, sign-in, or user identity system.
- **Has no telemetry and no crash reporting** — usage statistics, crash dumps, and diagnostics are never sent to any server.
- **Has no cloud backup or relay** — data exists only on the local machine and paired devices on your LAN.
- **Performs no online content fetching** — no lyrics, cover-art, or metadata lookups.
- **Keeps diagnostic logs on the local device only** (date-rotated, last 7 days); users may delete them at any time; Echo never uploads them.

### Future features

If a future version introduces **any capability that moves data off the local machine or LAN** — accounts, cloud backup/relay, telemetry, crash reporting, remote (non-LAN) sync, online metadata services — **this document must be updated before release**, stating what data is collected, the purpose, retention, user rights (view/export/delete), and how to opt out. Do not ship without that update.

## 4. Pre-release checklist

1. **Legal review:** have the project license (`LICENSE`, Echo Personal Study License) and the dependency distribution plan (per-platform `vendor/libmpv/*/manifest.json`, `NOTICE.md`, the bundled `THIRD-PARTY-LICENSES.txt`; `LICENSES.md` for Cargo/npm dependencies) reviewed by a lawyer familiar with software licensing and copyright. **Do not release without this.**
2. **License texts in packages:** confirm `THIRD-PARTY-LICENSES.txt` and the platform `NOTICE.md` are reachable inside all three installers (macOS `Contents/Resources/third-party/libmpv/`; Windows install root; Linux `/usr/share/doc/echo/`).
3. **Corresponding source reachable:** confirm the source links in each platform's NOTICE resolve at release time.
4. **Docs consistency:** this document and the "Media rights & privacy" sections of both READMEs match actual behavior; if this release adds any data-egress capability, confirm section 3's obligation was met.
