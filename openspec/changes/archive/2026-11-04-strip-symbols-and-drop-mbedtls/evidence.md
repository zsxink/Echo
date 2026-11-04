# issue #137 体积与符号基线 / 对比

记录于 2026-11-04，基线 HEAD `ae8e884`（#100 arm64-only 已合并），macOS 27.2 / arm64。

## strip 前基线（task 1.1）

- `node scripts/release/thin-macos-libmpv.mjs`（dry-run）：20 个库全部 `arm64` match，0 need thinning，0 hash mismatch，架构/哈希与 manifest 一致。
- vendor 目录：20 个 dylib，合计 **28,940,496 字节（27.60 MiB）**；其中三个 mbed 库合计 **1,447,648 字节（1.38 MiB）**。
- 本地符号计数口径：`nm -a <file> | awk '$2 ~ /^[tsbd]$/'`（小写本地符号）。
  - 主程序 `target/release/echo`：**32,509** 个（`t`/`s` 为 32,374）；`__LINKEDIT` vmsize `0x4c0000`（4.75 MiB）；文件大小 **25,971,840 字节**。
  - 已 strip（0）：`libavcodec`、`libavfilter`、`libavformat`、`libavutil`、`libswresample`、`libswscale`。
  - 待 strip（14 个 vendor + 3 个待删 mbed）：`libmpv` 5186、`libharfbuzz` 3814、`libglib` 3692、`libfreetype` 1612、`libmbedcrypto` 1415、`libpcre2` 961、`libass` 458、`libgraphite2` 317、`libmbedtls` 265、`libpng16` 246、`libintl` 182、`libmbedx509` 74、`libfribidi` 40、`libunibreak` 33。
- 现有产物：`target/release/echo` 25,971,840 字节；`target/release/bundle/dmg/Echo_0.3.1_aarch64.dmg` **25,519,020 字节**。
  - 注意：`target/release/bundle/macos/` 下当时只有两份 `rw.*.dmg` 临时镜像（各 91,267,072 字节），无 `Echo.app`；app bundle 由 5.1 的 Gate 1.9 重新构建后再测。

## strip / 删库后对比（task 5.3）

### 未压缩二进制（同一口径，实测）

| 对象 | strip 前 | strip 后 | 差值 |
|---|---:|---:|---:|
| `Echo` 主程序（release） | 25,971,840 B | **21,558,960 B** | −4,412,880 B（4.21 MiB） |
| `__LINKEDIT` vmsize | `0x4c0000`（4.75 MiB） | `0x88000`（0.53 MiB） | −4.22 MiB |
| 主程序本地符号（`t/s/b/d`） | 32,509 | **0** | 全清 |
| vendor dylib 合计 | 20 个 / 28,940,496 B（27.60 MiB） | **17 个 / 26,302,000 B（25.08 MiB）** | −2,638,496 B（2.52 MiB） |
| 其中：14 个待 strip 库 | 有本地符号 | **全部 0** | −1,183,936 B（1.13 MiB，与 issue 的 1.18 MB 同量级） |
| 其中：3 个 mbed 库 | 1,447,648 B（1.38 MiB） | **已删除** | −1,447,648 B |
| 6 个原本已 strip 的库 | 0 本地符号 | 字节不变（SHA-256 一致） | 0 |
| **主程序 + vendor 合计** | **54,912,336 B（52.36 MiB）** | **47,860,960 B（45.65 MiB）** | **−7,051,376 B（6.72 MiB）** |

> vendor strip 实测 1,427,936 B 含三个随后删除的 mbed 库；扣除后保留 17 库的净收益为 1,183,936 B。主程序收益（4.21 MiB）比 issue 预估的 3.84 MB 略高，因当前 release 构建产物与 issue 测量时的构建略有差异。

### 打包产物

| 对象 | strip 前 | strip 后 | 差值 |
|---|---:|---:|---:|
| `Echo.app`（`du -sh`） | 无基线（见下方说明） | **46 MiB** | — |
| DMG | 25,519,020 B（`target/release/bundle/dmg/`，strip 前 host 构建） | **22,638,316 B**（`target/aarch64-apple-darwin/release/bundle/dmg/`，strip 后 triple 构建） | −2,880,704 B（2.75 MiB，11.3%） |

> DMG 数值如实标注：两次产物分处 host 与 triple 路径，压缩后收益（2.75 MiB）明显小于未压缩二进制收益（6.72 MiB），符合 issue 的预期，但路径差异使其不是严格同条件对比。`Echo.app` 基线同样缺失（strip 前只有 `rw.*.dmg` 临时镜像），故只记录 strip 后的 46 MiB。

### 验证结果

- `pnpm verify:task -- 1.9 1.10 9.7`：**三项全部通过**（首次 1.9 因本机 `/Applications/Echo.app` 持有 `/tmp/com_zsxink_echo_si.sock` 而 `cold start exited early with 0`，用户退出该实例后重跑全绿；属环境干扰，非变更回归）。
- `codesign --verify --deep --strict`：通过；20 → 17 个 vendor dylib 逐个 `codesign --verify --strict` 通过。
- `otool -L` 依赖闭包：以 `libmpv.dylib` 为根恰好 17 个库，与 manifest / `bundle.macOS.files` 三方一致；三个 mbed 库不在闭包内；strip 前后各库引用列表完全相同。
- 精确匹配 `rg -n 'mbedtls|mbedcrypto|mbedx509|mbed TLS|Mbed-TLS'` 在活跃代码/配置/许可文件为空（change 规划文档与单测注入样本中的字样为描述与 fixture，非残留声明）；`openspec/changes/archive/**` 未改动。
- Gate 新增断言的注入自证：未 strip dylib、manifest/bundle 多余条目、三处一致但未引用库，均被拒绝并点名文件；`thin-macos-libmpv.test.mjs` 46 项全通过。

