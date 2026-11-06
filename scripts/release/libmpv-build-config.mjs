#!/usr/bin/env node
// Supply-chain configuration and repository paths for the pinned macOS
// libmpv/FFmpeg audio build.
//
// This module is data only: it declares the configure/meson option sets, the
// deployment-target pin, the dependency inventory and the fixed filesystem
// locations. Importing it has no side effects, so it is safe to share with the
// pure helpers (libmpv-build-pure.mjs), the build orchestration
// (libmpv-build-orchestration.mjs) and the public entry
// (build-macos-libmpv.mjs). The arrays here are single sources of truth: the
// build, the self-test and the Gate (task-1.10.mjs) all compare against them.

import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Both FFmpeg and mpv are configured with this prefix and installed through
// DESTDIR. Keeping it fixed means the embedded configure string and the
// install names recorded in consumers never depend on the work directory.
export const INSTALL_PREFIX = "/usr/local";

// The deployment target is a supply-chain property of the shipped binaries,
// not of the host that happened to run the build. Without this pin, macOS
// inherits the SDK/host version (27.2 on the current runner) and silently
// raises the minimum OS of every vendored dylib. The existing vendor baseline
// is 27.0; keeping it explicit makes a future host upgrade visible in diff.
export const MACOSX_DEPLOYMENT_TARGET = "27.0";

// libmpv 0.36 links libass and libswscale unconditionally (its build system
// requires both). They are not optional echo components: if the closure loses
// either, the build must fail instead of shipping a broken mpv.
export const MPV_HARD_DEPENDENCIES = ["libass.9.dylib", "libswscale.dylib"];

// Dylibs this build compiles itself. Homebrew auxiliary dylibs are prebuilt
// against their own deployment targets (some are 26.0), so only the files we
// compile must equal the pin exactly; every file must merely not be newer.
export const BUILT_DYLIBS = [
  "libmpv.dylib",
  "libavcodec.dylib",
  "libavformat.dylib",
  "libavutil.dylib",
  "libavfilter.dylib",
  "libswresample.dylib",
  "libswscale.dylib",
];

export const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
export const VENDOR = resolve(ROOT, "apps", "desktop", "src-tauri", "vendor", "libmpv", "macos");
export const MANIFEST_PATH = resolve(VENDOR, "manifest.json");
export const NOTICE_PATH = resolve(VENDOR, "NOTICE.md");
export const THIN_SCRIPT = resolve(ROOT, "scripts", "release", "thin-macos-libmpv.mjs");
export const PROBE_SCRIPT = resolve(ROOT, "scripts", "release", "ffmpeg-component-probe.mjs");

// FFmpeg is configured with a fixed prefix and installed through DESTDIR, so
// the embedded `avcodec_configuration()` does not change with the workdir.
// `--disable-everything` clears every component list but keeps the libraries
// mpv links (avcodec/avformat/avfilter/swscale/swresample). `--disable-avdevice`
// is not a component list, hence the explicit flag. Four graph endpoints
// (buffer/buffersink/abuffer/abuffersink) are registered unconditionally by
// FFmpeg 6.0 and are therefore absent from this list by design.
export const FFMPEG_CONFIGURE_ARGS = [
  `--prefix=${INSTALL_PREFIX}`,
  "--arch=arm64",
  "--target-os=darwin",
  "--enable-shared",
  "--disable-static",
  "--disable-everything",
  "--disable-autodetect",
  "--disable-programs",
  "--disable-doc",
  "--disable-debug",
  "--disable-avdevice",
  "--disable-network",
  "--disable-securetransport",
  "--disable-audiotoolbox",
  "--disable-videotoolbox",
  "--disable-zlib",
  // Pin the Mach-O minimum OS explicitly. MACOSX_DEPLOYMENT_TARGET is also
  // exported to configure/make/meson below, but the flag makes the intent part
  // of the embedded configure string the Gate compares against.
  `--extra-cflags=-mmacosx-version-min=${MACOSX_DEPLOYMENT_TARGET}`,
  // FFmpeg's macOS configure probes CoreFoundation/CoreMedia/CoreVideo
  // unconditionally and avutil_suggest links them into every dylib even when
  // videotoolbox is off; there is no --disable-corevideo option (configure
  // rejects it) and the framework probe re-enables them after option parsing.
  // No code path references their CONFIG_* symbols without videotoolbox, so
  // ask ld64 to drop load commands whose symbols are unused.
  `--extra-ldflags=-Wl,-dead_strip_dylibs -mmacosx-version-min=${MACOSX_DEPLOYMENT_TARGET}`,
  "--enable-small",
  "--enable-pthreads",
  "--enable-protocol=file",
  "--enable-demuxer=mp3,flac,ape,mov,ogg,wav",
  "--enable-decoder=mp3,mp3float,flac,ape,aac,aac_latm,alac,vorbis,opus,pcm_*",
  // FFmpeg 6.0's aac parser compiles aac_ac3_parser.o, which references
  // ff_ac3_find_syncword unconditionally; that symbol only exists under
  // CONFIG_AC3_PARSER. ELF tolerates the dangling reference, ld64 does not, so
  // the minimal macOS build must also enable the AC3 parser. It is audio-only
  // and adds no format outside the guaranteed set.
  "--enable-parser=aac,flac,mpegaudio,vorbis,opus,ac3",
  // aresample is not part of the audible effect chain, but mpv's lavfi
  // bridge needs it to auto-insert sample-format conversion when the `af`
  // graph starts from a different format (e.g. floatp → dblp). Without it
  // every effect node fails with "'aresample' filter not present".
  "--enable-filter=aformat,alimiter,equalizer,extrastereo,volume,aresample",
];

// mpv 0.36.0's meson_options.txt is the authority for these names. In
// particular there is no `videotoolbox-pl` in 0.36.0; passing it would abort
// meson setup. The build validates every -D name against the pinned source
// before configuring, so a copied option from a newer mpv fails loudly.
export const MPV_MESON_OPTIONS = [
  "-Dcplayer=false",
  "-Dlibmpv=true",
  "-Dgpl=false",
  "-Dbuild-date=false",
  "-Dtests=false",
  "-Dcoreaudio=enabled",
  "-Daudiounit=disabled",
  "-Dcocoa=disabled",
  "-Dgl=disabled",
  "-Dplain-gl=disabled",
  "-Dgl-cocoa=disabled",
  "-Dvideotoolbox-gl=disabled",
  "-Dvulkan=disabled",
  "-Dlibplacebo=disabled",
  "-Dlibplacebo-next=disabled",
  "-Dmacos-cocoa-cb=disabled",
  "-Dmacos-media-player=disabled",
  "-Dmacos-touchbar=disabled",
  "-Dlua=disabled",
  "-Djavascript=disabled",
  "-Dlibarchive=disabled",
  "-Dlibavdevice=disabled",
  "-Dzlib=disabled",
  "-Duchardet=disabled",
  "-Dlibbluray=disabled",
  "-Ddvdnav=disabled",
  "-Dcdda=disabled",
  "-Dsdl2=disabled",
  "-Dsdl2-audio=disabled",
  "-Dsdl2-video=disabled",
  "-Dsdl2-gamepad=disabled",
  "-Drubberband=disabled",
  "-Dlcms2=disabled",
  "-Dzimg=disabled",
  "-Dvector=disabled",
  "-Djpeg=disabled",
  "-Dvapoursynth=disabled",
  "-Dcplugins=disabled",
  "-Dswift-build=disabled",
  "-Dmanpage-build=disabled",
  "-Dhtml-build=disabled",
  "-Dpdf-build=disabled",
];

// Every non-FFmpeg/non-mpv dylib in the closure must be listed here. Failing
// closed is deliberate: an unmapped Homebrew library would otherwise enter the
// vendor tree without a recorded version or license row.
export const AUXILIARY_COMPONENTS = [
  { component: "libass", formula: "libass", file: "libass.9.dylib" },
  // FreeType is built with Brotli support, so both Brotli objects join the
  // closure even though Echo never renders WOFF fonts.
  { component: "Brotli common", formula: "brotli", file: "libbrotlicommon.1.dylib" },
  { component: "Brotli decoder", formula: "brotli", file: "libbrotlidec.1.dylib" },
  { component: "FreeType", formula: "freetype", file: "libfreetype.6.dylib" },
  { component: "FriBidi", formula: "fribidi", file: "libfribidi.0.dylib" },
  { component: "GLib", formula: "glib", file: "libglib-2.0.0.dylib" },
  { component: "Graphite2", formula: "graphite2", file: "libgraphite2.3.dylib" },
  { component: "HarfBuzz", formula: "harfbuzz", file: "libharfbuzz.0.dylib" },
  { component: "GNU libintl", formula: "gettext", file: "libintl.8.dylib" },
  { component: "PCRE2", formula: "pcre2", file: "libpcre2-8.0.dylib" },
  { component: "libpng", formula: "libpng", file: "libpng16.16.dylib" },
  { component: "libunibreak", formula: "libunibreak", file: "libunibreak.8.dylib" },
];

export const NOTICE_BEGIN = "<!-- BEGIN GENERATED AUXILIARY VERSIONS -->";
export const NOTICE_END = "<!-- END GENERATED AUXILIARY VERSIONS -->";
export const SYSTEM_PREFIXES = ["/usr/lib", "/System/Library", "/Library/Apple"];
