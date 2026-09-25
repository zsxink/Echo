# Bundled libmpv — Windows

This directory holds the pinned Windows playback runtime used by Echo's desktop
build: a static-FFmpeg `libmpv-2.dll` plus the Vulkan loader it eagerly imports.
It is not taken from a developer machine at build time.

| Field            | Value                                                                                    |
| ---------------- | ---------------------------------------------------------------------------------------- |
| Publisher        | [shinchiro/mpv-winbuild-cmake](https://github.com/shinchiro/mpv-winbuild-cmake)          |
| Release          | `20260814`                                                                               |
| Asset            | `mpv-dev-x86_64-20260814-git-7b8915bc1d.7z`                                              |
| Asset SHA-256    | `0af22b28e920620036d3ae08fd9283156dc9af0420bf4df84b0e02282094599c`                       |
| Architectures    | `x86_64`                                                                                 |
| libmpv ABI       | `2.5` (`libmpv-2.dll`)                                                                   |
| mpv build string | `mpv 20260814 git-7b8915bc1d` (static FFmpeg)                                            |
| Vulkan runtime   | LunarG VulkanRT `1.4.304.0` (`vulkan-1.dll`)                                             |

`libmpv-2.dll` uses a static (not delay-loaded) import of `vulkan-1.dll`, so the
Vulkan loader must ship alongside it. Both artifacts are pinned; the `files`
checksums in `manifest.json` are verified by the platform Gate.

Source and provenance:
- <https://github.com/shinchiro/mpv-winbuild-cmake/releases/tag/20260814>
- <https://sdk.lunarg.com/sdk/download/1.4.304.0/windows/VulkanRT-1.4.304.0-Components.zip>

Vulkan Loader license text is included as `VulkanRT-LICENSE.txt` in this
directory. The Windows distribution requirement (NOTICEItem uploaded with each
platform's installer) is covered by the `ci-release-pipeline` capability.