# Bundled libmpv — macOS audio-effects candidate

This is Echo's LGPL-only FFmpeg/libmpv build for the macOS arm64 release
target. Every bundled dylib is arm64-only: the arm64 slices use FFmpeg's
required audio filters and a small mpv patch that lets the player address a
named filter inside a libavfilter graph. Rebuilds must produce single-arch
binaries (`ARCHS=arm64`, `ONLY_ACTIVE_ARCH=YES`); universal/fat output is
rejected by the release Gate.

The `manifest.json` records binary checksums, per-file CPU architectures and
all bundled dependencies with their licenses. System Apple frameworks and
system `libiconv` are provided by macOS. libass and libswscale remain hard
dependencies of mpv (subtitle rendering and pixel-format conversion) and are
intentionally retained, not leftovers from the FFmpeg component set. The
generated table at the end of this file is the authoritative auxiliary
version record.

The audio-effects Gate is still open: this candidate has passed filter
availability and command-path probes with `ao=null`, which discards PCM. It
has not yet passed physical-output capture, peak/response measurements, loop
stress, latency/underrun, CPU-budget, or independent listening review.

## Corresponding source

The FFmpeg and mpv builds are LGPL (GPL and nonfree configure options are
disabled), so the corresponding source obligation is met by pointing at the
exact pinned sources recorded in `manifest.json`:

- FFmpeg 6.0: <https://ffmpeg.org/releases/ffmpeg-6.0.tar.xz>
  (SHA-256 in `manifest.json` `assetSha256`)
- mpv 0.36.0: <https://github.com/mpv-player/mpv/archive/refs/tags/v0.36.0.tar.gz>
  (SHA-256 in `manifest.json` `additionalSources`)
- Echo's two mpv patches: `scripts/release/patches/` in this repository
- Auxiliary libraries (Brotli, libass, FreeType, FriBidi, GLib, Graphite2,
  HarfBuzz, libintl, PCRE2, libpng, libunibreak): Homebrew builds on the build
  host; versions in the generated table below, sources at each component's
  upstream listed in `manifest.json` `licenses`

## License texts

The complete license texts for every license covering Echo's bundled player
binaries (GPL-2.0, LGPL-2.1, ISC, MIT, Apache-2.0, FTL, Zlib, BSD-3-Clause,
libpng) are in `THIRD-PARTY-LICENSES.txt` at the `vendor/libmpv/` level,
packaged inside the macOS app bundle under
`Contents/Resources/third-party/libmpv/`. System Apple frameworks and system
`libiconv` are provided by macOS and covered by Apple's own licensing.

<!-- BEGIN GENERATED AUXILIARY VERSIONS -->
| Component | Version / provenance |
| --- | --- |
| Brotli common | Homebrew 1.2.0 |
| Brotli decoder | Homebrew 1.2.0 |
| FreeType | Homebrew 2.15.0 |
| FriBidi | Homebrew 1.0.17 |
| GLib | Homebrew 2.90.1 |
| GNU libintl | Homebrew 1.0 |
| Graphite2 | Homebrew 1.3.15 |
| HarfBuzz | Homebrew 14.6.0 |
| libass | Homebrew 0.17.5_1 |
| libpng | Homebrew 1.6.59 |
| libunibreak | Homebrew 8.0 |
| PCRE2 | Homebrew 10.49 |
<!-- END GENERATED AUXILIARY VERSIONS -->
