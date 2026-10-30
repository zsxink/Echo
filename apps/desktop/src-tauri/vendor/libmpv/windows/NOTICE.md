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

## Licenses

The bundled binaries are licensed as follows (see `manifest.json` for the
machine-readable record):

| Component | License | Why |
| --- | --- | --- |
| `libmpv-2.dll` (mpv + static FFmpeg) | **GPL-2.0-or-later** | The shinchiro build configures FFmpeg with `--enable-gpl --enable-libx264 --enable-libx265` (`packages/ffmpeg.cmake`); x264/x265 are GPL libraries, so the statically linked FFmpeg — and the `libmpv-2.dll` containing it — is GPL, not LGPL. |
| `vulkan-1.dll` (Vulkan Loader) | Apache-2.0 | License text ships alongside as `VulkanRT-LICENSE.txt`. |

The complete license texts for every license covering Echo's bundled player
binaries (GPL-2.0, LGPL-2.1, ISC, MIT, Apache-2.0, FTL, Zlib, BSD-3-Clause,
libpng) are in `THIRD-PARTY-LICENSES.txt`, packaged with the Windows installer.

## Corresponding source

GPL-2.0-or-later requires that the corresponding source for the exact build be
obtainable:

- mpv: <https://github.com/mpv-player/mpv> (this build is `git-7b8915bc1d`,
  20260814; the shinchiro release page pins the exact commit)
- FFmpeg: <https://github.com/FFmpeg/FFmpeg> (built from the pinned submodule
  commit of the [shinchiro build scripts](https://github.com/shinchiro/mpv-winbuild-cmake),
  release `20260814`, which also carries x264/x265 and every other GPL
  component's build recipe)
- Build scripts: <https://github.com/shinchiro/mpv-winbuild-cmake/releases/tag/20260814>

The [shinchiro release asset](https://github.com/shinchiro/mpv-winbuild-cmake/releases/tag/20260814)
itself (`mpv-dev-x86_64-20260814-git-7b8915bc1d.7z`) is the pinned binary this
directory vendors; its SHA-256 is recorded above and in `manifest.json`.

## Provenance

- <https://github.com/shinchiro/mpv-winbuild-cmake/releases/tag/20260814>
- <https://sdk.lunarg.com/sdk/download/1.4.304.0/windows/VulkanRT-1.4.304.0-Components.zip>

The Windows distribution requirement (NOTICE files uploaded with each
platform's release) is covered by the `ci-release-pipeline` capability.
