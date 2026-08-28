# Bundled libmpv — macOS Gate

This directory is a pinned, redistributable **audio-default** libmpv runtime
used solely by Echo's macOS engineering Gate. It is not taken from a developer
machine at build time.

| Field            | Value                                                                             |
| ---------------- | --------------------------------------------------------------------------------- |
| Publisher        | [media-kit/libmpv-darwin-build](https://github.com/media-kit/libmpv-darwin-build) |
| Release          | `v0.7.2`                                                                          |
| Asset            | `libmpv-libs_v0.7.2_macos-universal-audio-default.tar.gz`                         |
| Asset SHA-256    | `2083852560cd1c4fabb1ca86b534bd3aaddebd4db965cb7110903f0604d31862`                |
| Architectures    | `arm64`, `x86_64`                                                                 |
| libmpv ABI       | `2.0.0` (`libmpv.dylib`)                                                          |
| mpv build string | `mpv 0.36.0`                                                                      |

The selected publisher describes the audio-default playback build as compatible
with commercial use. Its reproducible build recipe and release assets are
available in the publisher repository. The runtime contains libmpv and FFmpeg
libraries, subject to their upstream redistribution obligations, plus Mbed TLS
libraries. The candidate-release packaging task must replace this Gate notice
with the complete, versioned third-party notices before any public release.

Source and provenance: <https://github.com/media-kit/libmpv-darwin-build/releases/tag/v0.7.2>
