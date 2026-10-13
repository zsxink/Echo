# Bundled libmpv — macOS audio-effects candidate

This is Echo's LGPL-only FFmpeg/libmpv build. The arm64 slices use FFmpeg's
required audio filters and a small mpv patch that lets the player address a
named filter inside a libavfilter graph. The x86_64 slices in the original
universal libraries remain the previous mpv 0.36.0 / FFmpeg 6.0 runtime;
those Intel slices have not passed the audio-effects Gate yet. Auxiliary
subtitle-rendering libraries are arm64-only and are not loaded by the Intel
libmpv slice.

| Component | Version / provenance |
| --- | --- |
| FFmpeg | 6.0, source SHA-256 in `manifest.json`; LGPL build, no GPL/nonfree options |
| mpv/libmpv | 0.36.0, source SHA-256 in `manifest.json` |
| libass | Homebrew 0.17.5_1 |
| FreeType | Homebrew 2.14.3 |
| FriBidi | Homebrew 1.0.17 |
| GLib | Homebrew 2.90.0 |
| Graphite2 | Homebrew 1.3.15 |
| HarfBuzz | Homebrew 14.5.1 |
| GNU libintl | Homebrew gettext 1.0 |
| PCRE2 | Homebrew 10.49 |
| libpng | Homebrew 1.6.59 |
| libunibreak | Homebrew 8.0 |

The `manifest.json` records binary checksums and per-file CPU architectures.
All bundled dependencies and their licenses are inventoried there. System
Apple frameworks and system `libiconv` are provided by macOS.

The audio-effects Gate is still open: this candidate has passed filter
availability and command-path probes with `ao=null`, which discards PCM. It
has not yet passed physical-output capture, peak/response measurements, loop
stress, latency/underrun, CPU-budget, or independent listening review.
