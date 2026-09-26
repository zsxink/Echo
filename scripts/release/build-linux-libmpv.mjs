#!/usr/bin/env node
// Build the pinned Linux libmpv once inside a CI runner, so the vendored
// `vendor/libmpv/linux/` supply chain is a reproducible, glibc-bounded set.
//
// This script drives upstream mpv-build's build pipeline (libplacebo →
// libass → FFmpeg → mpv, via `./build`) but manages the component checkouts
// itself: it shallow-clones the four repos, pins each to the commit
// recorded in the Linux vendor manifest, and populates that repo's own
// submodules. mpv-build's own `./update` does a
// full-history `git fetch` of all four upstreams, which is far too heavy for
// CI — this script replaces only the checkout step, keeping mpv-build's
// `-config`/`-build` scripts (and their *_options files) untouched.
//
// The build is minimal and audio-only. FFmpeg is compiled SHARED so the
// runtime is `libmpv.so` + a small set of sibling `libav*.so` — the Linux
// analogue of macOS' libmpv.dylib / Windows' libmpv-2.dll. The FFmpeg
// component set is curated to exactly the formats the player smoke gate
// guarantees: mp3, flac, m4a/AAC, ogg/Vorbis, opus, wav.
//
// The manifest at apps/desktop/src-tauri/vendor/libmpv/linux/manifest.json is
// the source of truth for which refs to build. On a first run (no manifest
// yet) the script resolves each component's upstream HEAD and records it in
// the manifest it writes; every later build is pinned and refuses drift.
// Editing the pin means editing the manifest, then building again — never this
// script.
//
// Two deliberate deviations from stock mpv-build, both applied at build time
// to the checked-out mpv-build tree and recorded in build.tag:
//   1. scripts/ffmpeg-config hardcodes `--enable-static --disable-shared`,
//      and CLI options are PREPENDED — so the shared/shape can't be changed
//      via ffmpeg_options. The script patches that one default line to
//      `--disable-static --enable-shared`.
//   2. mpv-build compiles mpv but does NOT `meson install` it, so libmpv.so
//      lives in `mpv/build/` while libav*.so land in `build_libs/lib/`; both
//      trees are collected.
//
// Known runtime note: libass is built static here, so its font/text deps
// (fontconfig, harfbuzz, fribidi) are pulled into libmpv.so as SYSTEM
// DT_NEEDED entries — they are not vendored and must exist on the target
// distro (they do on any Ubuntu 22.04/24.04 desktop, and Tauri's deb pulls
// them via Depends). Only libmpv + libav*.so are shipped next to the exe.
//
// Usage (CI, Ubuntu 22.04 or newer):
//   node scripts/release/build-linux-libmpv.mjs --work <workdir> --out <outdir>
//
// A successful run leaves, in <outdir>: libmpv.so (symlink) + the versioned
// libmpv/libav* real files, build.tag, manifest.json, and NOTICE.md. The last
// two are what task 9.7's Linux gate and release.yml's NOTICE-staging step
// read, so the build is not usable without them. `--emit-provenance` rewrites
// just those two against an already-built tree (the path the self-test drives).
//
// Exit codes:
//   0 — libmpv.so + libav*.so built, rpath set, glibc ≤ 2.35 verified,
//       manifest.json + NOTICE.md written
//   1 — build, pin, or verification failure (details on stderr)
//
// Prerequisites on the host (install via apt before running):
//   build-essential git pkg-config python3 meson ninja-build patchelf
//   yasm nasm autoconf automake libtool libtool-bin
//     (FFmpeg asm, libass autogen; on Ubuntu the `libtool` package ships the
//      m4 macros and libtoolize, the `libtool` script this script checks on
//      PATH ships in libtool-bin — both are needed)
//   libfreetype-dev libfontconfig1-dev libharfbuzz-dev libfribidi-dev
//     (libass build-time deps — become system libmpv.so deps at runtime)
//   zlib1g-dev libunistring-dev libbz2-dev       (FFmpeg/libass build-time)
//   glslang-tools                                (libplacebo; build-time only)
//
// No libvulkan-dev needed: libplacebo's vulkan backend is disabled
// (libplacebo_options), matching mpv's own `-Dvulkan=disabled`.

import { mkdirSync } from "node:fs";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { readdirSync } from "node:fs";
import { lstatSync } from "node:fs";
import { realpathSync } from "node:fs";
import { rmSync } from "node:fs";
import { statSync } from "node:fs";
import { symlinkSync } from "node:fs";
import { writeFileSync } from "node:fs";
import { copyFileSync } from "node:fs";
import { resolve } from "node:path";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
const MANIFEST = resolve(ROOT, "apps/desktop/src-tauri/vendor/libmpv/linux/manifest.json");

// The glibc boundary Echo supports on Linux. Ubuntu 22.04 ships glibc 2.35;
// a library demanding a newer GLIBC_ symbol cannot run on 22.04/24.04 installs.
const GLIBC_MAX = 35;

// The seven libraries Echo ships and mpv pulls in transitively. Matches both
// the unversioned dev symlink and the versioned real file, so callers can pick
// which they want rather than each re-deriving the soname shape.
const SO_NAME = /^(lib(?:mpv|avcodec|avformat|avutil|swresample|swscale|avfilter))\.so(?:\.[0-9.]+)?$/;

// The four component repos mpv-build's build/update scripts expect as
// checkout dirs under the mpv-build tree. Names must match exactly:
// scripts/*-config cd into these.
const COMPONENTS = {
  libplacebo: "https://github.com/haasn/libplacebo.git",
  libass: "https://github.com/libass/libass.git",
  ffmpeg: "https://github.com/FFmpeg/FFmpeg.git",
  mpv: "https://github.com/mpv-player/mpv.git",
};

function fail(message) {
  process.stderr.write(`build-linux-libmpv: ${message}\n`);
  process.exit(1);
}

function parseArgs() {
  const args = process.argv.slice(2);
  const opts = { work: undefined, out: undefined, buildTag: undefined, emitOnly: false };
  for (let i = 0; i < args.length; i++) {
    if (args[i] === "--work") opts.work = resolve(args[++i]);
    else if (args[i] === "--out") opts.out = resolve(args[++i]);
    else if (args[i] === "--build-tag") opts.buildTag = args[++i];
    else if (args[i] === "--emit-provenance") opts.emitOnly = true;
    else if (args[i] === "--help" || args[i] === "-h") {
      process.stdout.write(
        "usage: build-linux-libmpv.mjs --work <workdir> --out <outdir>\n" +
        "       build-linux-libmpv.mjs --emit-provenance --out <outdir> --build-tag <json>\n",
      );
      process.exit(0);
    } else fail(`unknown argument: ${args[i]}`);
  }
  if (!opts.out) fail("missing --out <outdir> (where libmpv.so + libav*.so + build.tag land)");
  // --emit-provenance re-runs only the manifest/NOTICE writer against an
  // already-built tree; the buildTag argument carries the recorded build state
  // forward. --work is then not needed (nothing is compiled).
  if (!opts.emitOnly && !opts.work) fail("missing --work <workdir> (scratch for clones + build)");
  if (opts.emitOnly && !opts.buildTag) fail("--emit-provenance requires --build-tag <json>");
  return opts;
}

function run(cmd, args, opts = {}) {
  const r = spawnSync(cmd, args, { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"], ...opts });
  const out = `${r.stdout || ""}${r.stderr || ""}`;
  if (r.status !== 0) fail(`'${cmd} ${args.join(" ")}' failed (${r.status})\n${out}`);
  return out;
}

function exists(p) {
  try {
    return statSync(p).isFile();
  } catch {
    return false;
  }
}

function isDir(p) {
  try {
    return statSync(p).isDirectory();
  } catch {
    return false;
  }
}

function hasBin(name) {
  const r = spawnSync("sh", ["-c", `command -v ${name}`], { encoding: "utf8" });
  return r.status === 0;
}

// The Linux vendor manifest's commit fields are the build-time pins. If it is
// not written yet (first CI build), the script resolves each component's
// upstream HEAD and records it into build.tag so task 2.2 can backfill the
// manifest — after which every build is pinned and drift-checked.

function loadPins() {
  let raw = "";
  try {
    raw = readFileSync(MANIFEST, "utf8");
  } catch {
    return undefined;
  }
  try {
    const m = JSON.parse(raw);
    const picks = {};
    for (const k of ["mpvCommit", "ffmpegCommit", "libplaceboCommit", "libassCommit"]) {
      if (m[k]) picks[k] = m[k];
    }
    return { manifest: m, picks };
  } catch (error) {
    fail(`invalid manifest ${MANIFEST}: ${error.message}`);
  }
}

function componentPin(pins, name) {
  // manifest keys are "{component}Commit"; mpv-build checkout dirs are bare.
  return pins?.picks[`${name}Commit`];
}

// Clone mpv-build once (tiny) and ensure each component repo exists WITHOUT
// checking anything out — checkout is pinned right after. Uses blob:none
// partial clones so the metadata-only clone is cheap; the pinned commit's
// tree is fetched on demand.
function bootstrap(work, buildTag) {
  mkdirSync(work, { recursive: true });
  const mb = join(work, "mpv-build");
  if (!isDir(join(mb, ".git"))) {
    run("git", ["clone", "--quiet", "--depth", "1",
                "https://github.com/mpv-player/mpv-build.git", mb]);
  }
  // The mpv-build checkout itself is unpinned (master tip); record it so a
  // reproducible rebuild of a given build.tag is still auditable.
  buildTag.script = run("git", ["-C", mb, "rev-parse", "HEAD"]).trim();
  for (const [name, url] of Object.entries(COMPONENTS)) {
    const abs = join(mb, name);
    if (!isDir(abs)) {
      run("git", ["clone", "--quiet", "--filter=blob:none", "--no-checkout", url, abs]);
    }
  }
  return mb;
}

function pinComponent(buildDir, name, commit, buildTag) {
  const abs = join(buildDir, name);
  // Pinned manifest commit → fetch exactly that commit's tree; no manifest
  // (first build) → fetch the default branch tip and record it.
  const fetchArgs = ["-C", abs, "fetch", "--quiet", "--depth", "1", "origin"];
  if (commit) fetchArgs.push(commit);
  run("git", fetchArgs);
  run("git", ["-C", abs, "checkout", "--quiet", "--detach", "FETCH_HEAD"]);
  // Component repos carry their own submodules, and the build needs them
  // checked out. libplacebo's `src/opengl/include/glad/meson.build` errors
  // with "glad ... was not found in PYTHONPATH or `3rdparty`" when
  // `3rdparty/glad` is left unpopulated — and that path is reached
  // regardless of `-Dtests`/`-Ddemos`, so the GL headers it pulls in are not
  // optional here. The superproject checkout above never populates them.
  // `--init --recursive` fetches each submodule at the SHA the pinned
  // superproject records, so this stays reproducible; the shallow depth does
  // not propagate into submodules, which is the cost of that guarantee.
  run("git", ["-C", abs, "submodule", "update", "--init", "--recursive", "--quiet"]);
  const head = run("git", ["-C", abs, "rev-parse", "HEAD"]).trim();
  if (commit && head !== commit) {
    fail(`pin drift for ${name}: manifest says ${commit}, checkout is ${head}`);
  }
  buildTag.components[name] = { pin: commit ?? "first-build upstream HEAD", head };
}

// mpv-build ships a static-FFmpeg default that ffmpeg_config prepends AFTER
// ffmpeg_options (CLI last-wins), so the shared shape cannot be requested via
// the options file. Patch the single default line on the checked-out tree.
function patchFfmpegConfig(buildDir) {
  const f = join(buildDir, "scripts", "ffmpeg-config");
  const before = 'OPTIONS="--enable-gpl --disable-debug --disable-doc --enable-static --disable-shared --enable-pic"';
  const after = 'OPTIONS="--enable-gpl --disable-debug --disable-doc --disable-static --enable-shared --enable-pic"';
  const s = readFileSync(f, "utf8");
  if (!s.includes(before)) {
    fail(`unchanged since patching: ffmpeg-config no longer matches the mpv-build default being swapped (${f})`);
  }
  writeFileSync(f, s.replace(before, after));
  process.stdout.write("ok: patched scripts/ffmpeg-config to --disable-static --enable-shared\n");
}

function writeOptionFiles(buildDir) {
  // mpv: cplayer off, only the client library. Options present in mpv's
  // meson.options are passed through; libplacebo/libass are HARD dependencies
  // (meson dependency()) and have no option — the static builds satisfy them.
  // ABI must be 2.x to match MpvSys; -Dlibmpv=true is the switch, output name
  // and SONAME come from meson's version arg (libmpv.so.<client_api_version>).
  const mpvOptions = [
    "-Dcplayer=false",
    "-Dlibmpv=true",
    "-Dbuild-date=false",
    // Video: no VO backend, no hwaccel, no render context.
    "-Dgl=disabled",
    "-Dplain-gl=disabled",
    "-Dvulkan=disabled",
    "-Ddrm=disabled",
    "-Dwayland=disabled",
    "-Dx11=disabled",
    "-Ddmabuf-wayland=disabled",
    "-Dgbm=disabled",
    "-Dvaapi=disabled",
    "-Dvdpau=disabled",
    "-Degl=disabled",
    "-Dshaderc=disabled",
    "-Dsdl2-video=disabled",
    "-Dd3d11=disabled",
    "-Dvideotoolbox-gl=disabled",
    "-Dvideotoolbox-pl=disabled",
    // Audio sinks Echo never drives directly (ALSA/Pulse/PipeWire are host).
    "-Dalsa=disabled",
    "-Dpipewire=disabled",
    "-Dpulse=disabled",
    "-Djack=disabled",
    "-Doss-audio=disabled",
    "-Dsndio=disabled",
    "-Dopenal=disabled",
    "-Dsdl2-audio=disabled",
    // Subsystems that drag in lua/js/archive codecs.
    "-Dlua=disabled",
    "-Djavascript=disabled",
    "-Dlibarchive=disabled",
    "-Ddvdnav=disabled",
    "-Dlibbluray=disabled",
    "-Dcdda=disabled",
    "-Dlibavdevice=disabled",
  ];
  writeFileSync(join(buildDir, "mpv_options"), mpvOptions.join("\n") + "\n");

  // FFmpeg: curated component set covering exactly the smoke-gate formats
  // (mp3, flac, m4a/AAC, ogg/Vorbis, opus, wav). --disable-everything strips
  // programs/protocols/filters; the patched ffmpeg-config default adds
  // --disable-static --enable-shared --enable-pic. All decoders are FFmpeg's
  // native ones — no external codec libraries.
  const ffmpegOptions = [
    "--disable-everything",
    "--disable-programs",
    "--disable-avdevice",
    "--disable-doc",
    "--enable-protocol=file",
    "--enable-demuxer=flac,mov,mp3,ogg,wav",
    "--enable-decoder=flac,aac,aac_latm,mp3,mp3float,vorbis,opus,pcm_*",
    "--enable-parser=aac,flac,mpegaudio,vorbis,opus",
    "--enable-small",
  ];
  writeFileSync(join(buildDir, "ffmpeg_options"), ffmpegOptions.join("\n") + "\n");

  // libplacebo: keep mpv-build's static defaults, turn off the GPU backends
  // so the host needs no vulkan/GL headers and nothing GPU-y runs at runtime.
  // This is what mpv's own -Dvulkan=disabled refers to on the mpv side.
  writeFileSync(join(buildDir, "libplacebo_options"), "-Dvulkan=disabled\n");

  process.stdout.write(
    `wrote mpv_options (${mpvOptions.length}) + ffmpeg_options (${ffmpegOptions.length}) + libplacebo_options\n`,
  );
}

function build(buildDir) {
  for (const tool of [
    "meson", "ninja", "pkg-config", "python3",
    "yasm", "nasm", "patchelf",               // ffmpeg asm, rpath
    "autoreconf", "automake", "libtool",      // libass autogen
  ]) {
    if (!hasBin(tool)) fail(`missing build prerequisite '${tool}' — install it via apt first`);
  }
  // ./build runs libplacebo → libass → ffmpeg → mpv in sequence.
  run("sh", ["./build"], { cwd: buildDir, env: process.env, maxBuffer: 64 * 1024 * 1024 });
}

// Picks the shipped shared objects out of one build directory, as absolute
// paths to the real versioned files.
//
// Match on soname, not `endsWith(".so")`. The real files are versioned
// (libmpv.so.2.5, libavcodec.so.61) and so never end in ".so"; the only names
// that do are the unversioned dev symlinks meson installs beside them. So a
// plain `.endsWith(".so")` filter selected nothing at all and the `wanted` check
// downstream failed on every Linux build. Dirent.isFile() is no-follow, so
// those symlinks are skipped and the real versioned file beside them is
// collected exactly once, under its own name — no blob is recorded twice.
//
// Exported (and pure apart from reading the directory) so the self-test can
// drive it directly: it is unreachable from the script's own CLI, which has to
// clone and compile ten-toolchains' worth of real sources first.
export function realSharedObjectsIn(dir) {
  if (!isDir(dir)) return [];
  const found = [];
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    if (e.isFile() && SO_NAME.test(e.name)) found.push(realpathSync(join(dir, e.name)));
  }
  return found;
}

function collectArtifacts(buildDir, out, _buildTag) {
  // libplacebo/libass/ffmpeg are `meson/make install`ed into build_libs/lib;
  // mpv is only compiled (`mpv-build` does no install), so libmpv.so lives in
  // mpv/build. Merge both trees' real .so files into the vendor dir.
  const libDirs = [join(buildDir, "build_libs", "lib"), join(buildDir, "mpv", "build")];
  const uniq = [...new Set(libDirs.flatMap(realSharedObjectsIn))];

  const wanted = ["libmpv", "libavcodec", "libavformat", "libavutil",
                  "libswresample", "libswscale", "libavfilter"];
  if (uniq.length === 0) {
    // The generic `expected <lib>.so produced` message below cannot tell
    // "this one library is missing" apart from "the filter matched nothing",
    // which is the failure that actually happened here and the one that cost a
    // release run. Say which, and what was there.
    const seen = libDirs.flatMap((d) =>
      isDir(d) ? readdirSync(d).slice(0, 20).map((n) => `${d}/${n}`) : [],
    );
    fail(
      `no real .so files collected from ${libDirs.join(" or ")}` +
      (seen.length ? `; found: ${seen.join(", ")}` : " (neither directory exists)"),
    );
  }
  const present = new Set(uniq.map((p) => p.split("/").pop().split(".")[0]));
  for (const w of wanted) {
    if (!present.has(w)) fail(`expected ${w}.so produced, not in ${libDirs.join(" or ")}`);
  }

  rmSync(out, { recursive: true, force: true });
  mkdirSync(out, { recursive: true });

  // Copy each real file keeping its versioned basename (libavcodec.so.61,
  // libmpv.so.2.5) — the loader resolves DT_NEEDED by SONAME, so the vendor
  // tree must carry the versioned real files.
  for (const real of uniq) {
    copyFileSync(real, join(out, real.split("/").pop()));
  }

  // Then offer an unversioned libmpv.so at the path Echo loads next to the
  // exe, as a symlink to the versioned real file (no duplicated blob).
  const libmpvReal = libmpvRealFileIn(uniq);
  if (!libmpvReal) fail("libmpv real file (libmpv.so.<ver>) not produced");
  symlinkSync(libmpvReal.split("/").pop(), join(out, "libmpv.so"));

  process.stdout.write(`ok: collected ${uniq.length} real .so + libmpv.so symlink\n`);
  return uniq.length;
}

function libmpvRealFileIn(files) {
  return files.find((p) => {
    const n = p.split("/").pop();
    return n.startsWith("libmpv.so.") && statSync(p).isFile();
  });
}

function setRpath(out) {
  // Versioned real files carry DT_NEEDED resolution; patchelf acts on real
  // files only (the libmpv.so symlink needs no rpath of its own).
  for (const name of realSoFiles(out)) {
    run("patchelf", ["--set-rpath", "$ORIGIN", join(out, name)]);
  }
  process.stdout.write("ok: patchelf set $ORIGIN rpath on vendored .so files\n");
}

// Highest GLIBC_x.y symbol version across a set of `readelf --version-info`
// dumps, as `{ lib, ver }`. Kept separate from checkGlibc (and exported) so the
// scan is testable without readelf or real ELF: it was here that `const
// highest` threw "Assignment to constant variable" on the first symbol it
// found, a line no run had ever executed because artifact collection always
// failed first.
export function highestGlibcSymbol(dumps) {
  let highest = { lib: null, ver: [2, 0, 0] };
  for (const { lib, dump } of dumps) {
    for (const m of dump.matchAll(/GLIBC_([0-9]+)\.([0-9]+)(?:\.([0-9]+))?/g)) {
      const v = [Number(m[1]), Number(m[2]), m[3] ? Number(m[3]) : 0];
      if (cmpVersion(v, highest.ver) > 0) highest = { lib, ver: v };
    }
  }
  return highest;
}

// Verifies the glibc ceiling and returns it, so the manifest can record the
// measured bound rather than the assumed one.
function checkGlibc(out) {
  // readelf --version-info lists the GLIBC_x.y version each undefined symbol
  // requires. Versioned libs (libavcodec.so.61) define the real demand; the
  // libmpv.so symlink resolves to the same blob. Symbol versions embed the
  // build host's glibc, so a 2.36+ distro would surface here and fail.
  const dumps = realSoFiles(out).map((name) => ({
    lib: name,
    dump: run("readelf", ["--version-info", join(out, name)]),
  }));
  const highest = highestGlibcSymbol(dumps);
  if (cmpVersion(highest.ver, [2, GLIBC_MAX, 0]) > 0) {
    fail(`glibc requirement ${highest.ver.join(".")} (${highest.lib}) > 2.${GLIBC_MAX}`);
  }
  process.stdout.write(`ok: glibc ≤ 2.${GLIBC_MAX} (highest GLIBC_${highest.ver.join(".")} in ${highest.lib})\n`);
  // task-9.7 re-derives this from readelf; the manifest records it as the
  // documented ceiling so a manifest and a Gate run can never disagree.
  return Number(highest.ver[1]);
}

function cmpVersion(a, b) {
  for (let i = 0; i < 3; i++) {
    if (a[i] !== b[i]) return a[i] > b[i] ? 1 : -1;
  }
  return 0;
}

function recordBuildTag(out, buildTag) {
  const lines = [
    "mpv-build script checkout:",
    `  head=${buildTag.script} (mpv-build master tip, unpinned)`,
    "pinned components (pin / checkout head):",
    ...Object.entries(buildTag.components).map(
      ([name, c]) => `  ${name}: pin=${c.pin} head=${c.head}`,
    ),
    "build flags: minimal (audio-only) — libplacebo vulkan=disabled, ffmpeg shared+curated codecs, mpv cplayer=off",
    `library count: ${buildTag.libs}`,
    `glibc ceiling: 2.${GLIBC_MAX} (build host must be Ubuntu 22.04 or newer)`,
    `built by: ${process.env.GITHUB_RUN_ID ? `github-actions run ${process.env.GITHUB_RUN_ID}` : "local"}`,
  ];
  writeFileSync(join(out, "build.tag"), lines.join("\n") + "\n");
  process.stdout.write(
    `wrote build.tag (${Object.keys(buildTag.components).length} components, ${buildTag.libs} libs)\n`,
  );
}

// --- provenance: manifest.json + NOTICE.md ---------------------------------
//
// Unlike macOS/Windows (a downloaded upstream asset described by an
// `asset` + `assetSha256`), the Linux runtime is built from source here, so
// provenance is the four component commits plus the per-file digests of what
// was actually produced. `loadPins` reads back the commit fields written below,
// so this file is both the record and the next build's input — task 9.7's
// Linux gate re-verifies every digest and the glibc ceiling against it.

const LICENSES = [
  ["mpv", "GPL-2.0-or-later", "https://github.com/mpv-player/mpv"],
  ["FFmpeg", "LGPL-2.1-or-later", "https://github.com/FFmpeg/FFmpeg"],
  ["libplacebo", "LGPL-2.1-only", "https://github.com/haasn/libplacebo"],
  ["libass", "ISC", "https://github.com/libass/libass"],
  ["freetype", "FTL", "https://gitlab.freedesktop.org/freetype/freetype"],
  ["fontconfig", "MIT", "https://gitlab.freedesktop.org/fontconfig/fontconfig"],
  ["harfbuzz", "MIT", "https://github.com/harfbuzz/harfbuzz"],
  ["fribidi", "LGPL-2.1-or-later", "https://github.com/fribidi/fribidi"],
];

// The self-build links these in as *system* libraries (they are libass build
// deps, recorded above precisely so the NOTICE covers what the build pulls in).
// Naming them here keeps that runtime obligation visible to whoever ships the
// deb/AppImage.
const SYSTEM_DEPS = ["freetype", "fontconfig", "harfbuzz", "fribidi"];

function sha256Of(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

// Every shipped shared object that is a real file. Must be lstat, not stat:
// exists() below follows symlinks, which would admit the libmpv.so link and
// hash its target under both names.
function realSoFiles(out) {
  return readdirSync(out).filter((n) => {
    if (!SO_NAME.test(n)) return false;
    try {
      return lstatSync(join(out, n)).isFile();
    } catch {
      return false;
    }
  });
}

// task-9.7's 7a re-hashes every files{} entry on each Gate run; an entry that
// is stale, or a library missing from the manifest, must fail HERE first — a
// manifest that only covers some of the shipped .so would let task-9.7's glibc
// sweep and the Gate's own checks disagree about what the vendor tree holds.
function provenanceFiles(out) {
  const found = new Map();
  for (const name of realSoFiles(out)) {
    const base = name.split(".")[0];
    // Prefer the versioned real file; never downgrade to the bare .so name.
    if (found.has(base) && !found.get(base).name.includes(".so.")) continue;
    found.set(base, { name, digest: sha256Of(join(out, name)) });
  }
  for (const base of ["libmpv", "libavcodec", "libavformat", "libavutil",
                      "libswresample", "libswscale", "libavfilter"]) {
    if (!found.has(base)) {
      fail(`vendor tree is missing ${base}.so — manifest.json cannot be written from an incomplete build`);
    }
  }
  return Object.fromEntries([...found].map(([base, f]) => [f.name, f.digest]));
}

function resolveGlibcMax() {
  // Normal build path: checkGlibc() already measured and returned it.
  // --emit-provenance path: it re-writes the provenance documents for a tree
  // someone else built, so the ceiling recorded here is the contract that tree
  // was verified against, not a fresh measurement. Never re-run readelf here:
  // this mode exists precisely to work on a vendor tree the current host may
  // not be able to introspect, and --emit-provenance's only caller is the
  // provenance self-test, whose fixtures are text blobs rather than ELF. The
  // measured check stays where it is load-bearing, in the real build path,
  // where release.yml runs it against the freshly compiled libraries.
  return GLIBC_MAX;
}

function emitProvenance(out, buildTag, glibcMax) {
  const files = provenanceFiles(out);
  const headOf = (n) => buildTag.components?.[n]?.head;

  const manifest = {
    schemaVersion: 1,
    platform: "linux",
    kind: "self-built-audio-only",
    mpvCommit: headOf("mpv"),
    ffmpegCommit: headOf("ffmpeg"),
    libplaceboCommit: headOf("libplacebo"),
    libassCommit: headOf("libass"),
    mpvBuildScript: buildTag.script ?? null,
    libmpvAbi: libmpvAbiOf(out),
    architectures: ["x86_64"],
    glibcMax,
    systemLibraryDependencies: SYSTEM_DEPS,
    licenses: LICENSES.map(([component, license, source]) => ({
      component,
      license,
      source,
    })),
    files,
  };
  // 2-space + trailing newline, matching the macOS/Windows vendor manifests.
  writeFileSync(join(out, "manifest.json"), JSON.stringify(manifest, null, 2) + "\n");
  process.stdout.write(
    `wrote manifest.json (${Object.keys(files).length} files, glibc ≤ 2.${glibcMax})\n`,
  );

  writeFileSync(join(out, "NOTICE.md"), renderNotice(manifest));
  process.stdout.write("wrote NOTICE.md\n");
}

function libmpvAbiOf(out) {
  const name = readdirSync(out).find((n) => n.startsWith("libmpv.so."));
  return name ? name.slice("libmpv.so.".length) : null;
}

function renderNotice(manifest) {
  const pin = (c) => c ?? "unrecorded";
  return `# Bundled libmpv — Linux

Echo ships a self-built \`libmpv.so\` and its FFmpeg siblings. Unlike the macOS
and Windows builds, nothing here is downloaded from an upstream release
asset: \`scripts/release/build-linux-libmpv.mjs\` compiles mpv and FFmpeg from
the exact commits below, so the provenance is the source revision rather than
a published tarball.

| Field | Value |
| --- | --- |
| Kind | ${manifest.kind} |
| Architecture | ${manifest.architectures.join(", ")} |
| libmpv ABI | ${manifest.libmpvAbi ?? "unrecorded"} |
| mpv commit | ${pin(manifest.mpvCommit)} |
| FFmpeg commit | ${pin(manifest.ffmpegCommit)} |
| libplacebo commit | ${pin(manifest.libplaceboCommit)} |
| libass commit | ${pin(manifest.libassCommit)} |
| mpv-build script | ${manifest.mpvBuildScript ?? "unrecorded"} |
| glibc ceiling | 2.${manifest.glibcMax} (Ubuntu 22.04 or newer) |
| Bundled files | ${Object.keys(manifest.files).length} — digests in \`manifest.json\` |

## Licenses

The bundled libraries are licensed as follows. mpv itself is GPL-2.0-or-later
and is redistributed here unmodified as a dynamically linked shared library,
so the corresponding source for that exact commit is the upstream checkout
named in the table above.

${manifest.licenses.map((l) => `- **${l.component}** — ${l.license} — ${l.source}`).join("\n")}

## System libraries required at runtime

libass is linked statically, so its font/text stack comes in as *system*
dependencies rather than bundled files: ${SYSTEM_DEPS.join(", ")}. These are
resolved from the host distribution (present on any Ubuntu 22.04/24.04 desktop)
and are declared in the package's dependencies.

## Rebuilding

    node scripts/release/build-linux-libmpv.mjs \\
      --work <workdir> --out apps/desktop/src-tauri/vendor/libmpv/linux

Build on Ubuntu 22.04 or newer. A host with glibc newer than 2.${manifest.glibcMax}
produces binaries that will not run on 22.04, and the script fails rather than
writing them. Each file's SHA-256 in \`manifest.json\` is the value the release
Gate re-verifies before packaging.
`;
}

// Imported by the self-test for realSharedObjectsIn(); running the full build
// on import would clone and compile four repos, so gate the entry point the
// same way scripts/verify/spec-scenarios.mjs does.
if (import.meta.url === `file://${process.argv[1]}`) main();

function main() {
  const opts = parseArgs();

  // --emit-provenance: the tree is already built, just (re)write the two
  // documents the rest of the pipeline reads. Skips clone/build/readelf so it
  // is also the path the self-test drives.
  if (opts.emitOnly) {
    emitProvenance(opts.out, JSON.parse(opts.buildTag), resolveGlibcMax());
    return;
  }

  const pins = loadPins();
  if (pins) {
    process.stdout.write(`pinning from ${MANIFEST}\n`);
  } else {
    process.stdout.write("no manifest yet — resolving upstream HEADs into build.tag\n");
  }

  // Never leave a stale vendored tree around from a half-built run.
  const buildTag = { script: null, components: {}, libs: 0 };

  const buildDir = bootstrap(opts.work, buildTag);
  for (const name of Object.keys(COMPONENTS)) {
    pinComponent(buildDir, name, componentPin(pins, name), buildTag);
  }

  patchFfmpegConfig(buildDir);
  writeOptionFiles(buildDir);
  build(buildDir);
  buildTag.libs = collectArtifacts(buildDir, opts.out, buildTag);
  setRpath(opts.out);
  buildTag.glibcMax = checkGlibc(opts.out);
  recordBuildTag(opts.out, buildTag);
  // Last, so it hashes the finished post-processed (rpath-patched) tree.
  emitProvenance(opts.out, buildTag, buildTag.glibcMax);

  process.stdout.write(`ok: Linux libmpv built and copied to ${opts.out}\n`);
}