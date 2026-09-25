# Windows libmpv — pinned provenance

Echo 的 Windows 播放后端由两个固定来源的构件组成，均在构建/发布时可审计。

## libmpv-2.dll

| Field       | Value                                                           |
| ----------- | --------------------------------------------------------------- |
| Publisher   | [shinchiro/mpv-winbuild-cmake](https://github.com/shinchiro/mpv-winbuild-cmake) |
| Release     | `20260814`                                                      |
| Asset       | `mpv-dev-x86_64-20260814-git-7b8915bc1d.7z`                     |
| Asset SHA-256 | `0af22b28e920620036d3ae08fd9283156dc9af0420bf4df84b0e02282094599c` |
| Architecture| x86-64 (PE32+)                                                  |
| mpv build   | `mpv 20260814 git-7b8915bc1d`（静态链接 FFmpeg，单文件运行时） |
| Redistribution | LGPL-2.1-or-later（上游 mpv / FFmpeg）                         |

> 注：此资产是 shinchiro 的 **dev** 包，提供一个静态链接 FFmpeg 的 `libmpv-2.dll`
> （导出完整 `mpv_*` C API，ABI 与 Echo 的 `MpvSys` 符号集一致）。不包含
> `mpv.exe` / `libav*.dll`。来源固定，不追最新。

## vulkan-1.dll

| Field       | Value                                                          |
| ----------- | -------------------------------------------------------------- |
| Publisher   | [LunarG / KhronosGroup Vulkan-Loader](https://www.lunarg.com/)  |
| Runtime     | VulkanRT 1.4.304.0                                             |
| Asset       | `VulkanRT-1.4.304.0-Components.zip` → `x64/vulkan-1.dll`       |
| Asset URL   | `https://sdk.lunarg.com/sdk/download/1.4.304.0/windows/VulkanRT-1.4.304.0-Components.zip` |
| DLL SHA-256 | `c7ee7f7edeb5f2c175effa60621b33fc4b6d3e8764d03c9491c77d754bd804f1` |
| Architecture | x86-64 (PE32+)                                                |
| License     | Apache-2.0 / MIT 组件，全文见 `VulkanRT-LICENSE.txt`           |

> `libmpv-2.dll` 在 Windows 上 **eager-import** `vulkan-1.dll`（无 delay-load），
> 因此必须与 Vulkan loader 一起分发，否则 `Library::new` 在缺 loader 的
> 精简环境加载失败。版本固定为 1.4.304.0，后续升级需重新生成 manifest 并回填本表。
