#!/usr/bin/env node
// Cross-platform self-test for build-macos-libmpv.mjs.
//
// Everything here avoids the macOS-only build steps: argument parsing, pin
// verification, meson-option validation, configure-warning scanning,
// pkg-config rewriting, install-name planning and provenance writers are pure.
// The one real-file check (the current vendor libmpv reference plan) runs only
// on macOS, where otool exists.

import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  writeFileSync,
} from "node:fs";
import { homedir, tmpdir } from "node:os";
import { resolve } from "node:path";
import {
  FFMPEG_CONFIGURE_ARGS,
  MACOSX_DEPLOYMENT_TARGET,
  MPV_HARD_DEPENDENCIES,
  MPV_MESON_OPTIONS,
  applyBuildRecords,
  assertHost,
  assertPatchResult,
  assertSafeScratchDir,
  backupVendorTree,
  classifyDependencyPath,
  compareDottedVersions,
  deploymentTargetViolation,
  findConfigureWarnings,
  findUnknownMesonOptions,
  homebrewVersionFromSource,
  mergeNoticeBlock,
  normalizeDylibName,
  optionName,
  parseArgs,
  parseMesonOptionNames,
  parseMinos,
  parseOtoolLibraries,
  planInstallNameChanges,
  renderNoticeBlock,
  resolvePatchPaths,
  restoreVendorTree,
  rewritePkgConfigPrefix,
  summarizeFfmpegBuild,
  summarizeMpvBuild,
  verifyPinnedDigest,
} from "./build-macos-libmpv.mjs";

function sha256(path) {
  return createHash("sha256").update(readFileSync(resolve(path))).digest("hex");
}

let failures = 0;
function assert(cond, label, detail) {
  if (cond) {
    process.stdout.write(`ok   ${label}\n`);
  } else {
    process.stderr.write(`FAIL ${label}\n`);
    if (detail) process.stderr.write(`     ${detail}\n`);
    failures += 1;
  }
}

function throws(fn) {
  try {
    fn();
  } catch (error) {
    return error;
  }
  return null;
}

function equal(actual, expected) {
  return JSON.stringify(actual) === JSON.stringify(expected);
}

// A faithful excerpt of mpv v0.36.0/meson_options.txt. The build must reject
// any -D name not declared here; -Dvideotoolbox-pl is the concrete drift this
// change fixes (0.36.0 only has videotoolbox-gl).
const MPV_036_OPTIONS = `
option('cplayer', type: 'boolean', value: true, description: 'mpv CLI player')
option('libmpv', type: 'boolean', value: false, description: 'shared library')
option('gpl', type: 'boolean', value: false, description: 'GPL components')
option('build-date', type: 'boolean', value: true, description: 'whether to include build date')
option('tests', type: 'boolean', value: true, description: 'unit tests')
option('cocoa', type: 'feature', value: 'auto', description: 'Cocoa')
option('coreaudio', type: 'feature', value: 'auto', description: 'CoreAudio')
option('audiounit', type: 'feature', value: 'auto', description: 'AudioUnit')
option('gl', type: 'feature', value: 'auto', description: 'OpenGL')
option('plain-gl', type: 'feature', value: 'auto', description: 'OpenGL without GLX/X11')
option('gl-cocoa', type: 'feature', value: 'auto', description: 'Cocoa OpenGL')
option('videotoolbox-gl', type: 'feature', value: 'auto', description: 'VideoToolbox GL')
option('vulkan', type: 'feature', value: 'auto', description: 'Vulkan')
option('libplacebo', type: 'feature', value: 'auto', description: 'libplacebo')
option('libplacebo-next', type: 'feature', value: 'auto', description: 'libplacebo-next')
option('macos-cocoa-cb', type: 'feature', value: 'auto', description: 'macOS Cocoa callbacks')
option('macos-media-player', type: 'feature', value: 'auto', description: 'macOS media player')
option('macos-touchbar', type: 'feature', value: 'auto', description: 'macOS Touch Bar')
option('lua', type: 'feature', value: 'auto', description: 'Lua')
option('javascript', type: 'feature', value: 'auto', description: 'Javascript')
option('libarchive', type: 'feature', value: 'auto', description: 'libarchive')
option('libavdevice', type: 'feature', value: 'auto', description: 'libavdevice')
option('zlib', type: 'feature', value: 'auto', description: 'zlib')
option('uchardet', type: 'feature', value: 'auto', description: 'uchardet')
option('libbluray', type: 'feature', value: 'auto', description: 'libbluray')
option('dvdnav', type: 'feature', value: 'auto', description: 'dvdnav')
option('cdda', type: 'feature', value: 'auto', description: 'cdda')
option('sdl2', type: 'feature', value: 'auto', description: 'SDL2')
option('sdl2-audio', type: 'feature', value: 'auto', description: 'SDL2 audio')
option('sdl2-video', type: 'feature', value: 'auto', description: 'SDL2 video')
option('sdl2-gamepad', type: 'feature', value: 'auto', description: 'SDL2 gamepad')
option('rubberband', type: 'feature', value: 'auto', description: 'rubberband')
option('lcms2', type: 'feature', value: 'auto', description: 'lcms2')
option('zimg', type: 'feature', value: 'auto', description: 'zimg')
option('vector', type: 'feature', value: 'auto', description: 'vector')
option('jpeg', type: 'feature', value: 'auto', description: 'jpeg')
option('vapoursynth', type: 'feature', value: 'auto', description: 'VapourSynth')
option('cplugins', type: 'feature', value: 'auto', description: 'C plugins')
option('swift-build', type: 'feature', value: 'auto', description: 'Swift build')
option('manpage-build', type: 'feature', value: 'auto', description: 'manpage')
option('html-build', type: 'feature', value: 'auto', description: 'html manual')
option('pdf-build', type: 'feature', value: 'auto', description: 'pdf manual')
`;

// --- parseArgs --------------------------------------------------------------
{
  const opts = parseArgs(["--work", "/tmp/work", "--out", "/tmp/out"]);
  assert(
    opts.work === resolve("/tmp/work") && opts.out === resolve("/tmp/out") && !opts.writeVendor,
    "build mode parses --work/--out",
    JSON.stringify(opts),
  );
  const vendor = parseArgs(["--out", "/tmp/out", "--write-vendor"]);
  assert(vendor.writeVendor && vendor.work === undefined, "--write-vendor does not require --work");
  assert(parseArgs(["--help"]).help, "--help parses without paths");
  assert(throws(() => parseArgs(["--out"])) !== null, "--out without a value is rejected");
  assert(throws(() => parseArgs(["--work", "/tmp/work"])) !== null, "missing --out is rejected");
  assert(throws(() => parseArgs(["--bogus"])) !== null, "unknown arguments are rejected");
}

// --- host guard -------------------------------------------------------------
{
  assertHost("darwin", "arm64");
  const wrongPlatform = throws(() => assertHost("linux", "arm64"));
  assert(
    wrongPlatform && /darwin\/arm64/.test(wrongPlatform.message),
    "non-macOS hosts are rejected with the expected platform in the message",
    wrongPlatform?.message,
  );
  assert(throws(() => assertHost("darwin", "x64")) !== null, "x86_64 hosts are rejected");
}

// --- pin verification -------------------------------------------------------
{
  const expected = "a".repeat(64);
  verifyPinnedDigest({ name: "ffmpeg.tar.xz", expected, actual: expected });
  const mismatch = throws(() =>
    verifyPinnedDigest({ name: "ffmpeg.tar.xz", expected, actual: "b".repeat(64) }),
  );
  assert(
    mismatch && mismatch.message.includes("ffmpeg.tar.xz") && mismatch.message.includes("SHA-256"),
    "a digest mismatch fails and names the archive",
    mismatch?.message,
  );
  assert(throws(() => verifyPinnedDigest({ name: "x", expected: "nope", actual: "nope" })) !== null, "a malformed pin is rejected");
}

// --- patch pins -------------------------------------------------------------
{
  const patchA = "scripts/release/patches/mpv-lavfi-target-command.patch";
  const patchB = "scripts/release/patches/mpv-fix-missing-objc.patch";
  const patches = resolvePatchPaths([
    { path: patchA, sha256: sha256(patchA) },
    { path: patchB, sha256: sha256(patchB) },
  ]);
  assert(
    patches.length === 2 &&
      patches[0].relativePath === patchA &&
      patches[1].relativePath === patchB &&
      patches.every((entry) => entry.path.includes("patches")),
    "manifest patch paths resolve in order",
  );

  const missing = throws(() => resolvePatchPaths([{ path: "scripts/release/patches/does-not-exist.patch" }]));
  assert(
    missing && missing.message.includes("does-not-exist.patch"),
    "a missing pinned patch fails and names the file",
    missing?.message,
  );

  const wrongHash = throws(() => resolvePatchPaths([{ path: patchA, sha256: "0".repeat(64) }]));
  assert(
    wrongHash && wrongHash.message.includes(patchA) && wrongHash.message.includes("SHA-256"),
    "a patch whose content does not match its pin is rejected",
    wrongHash?.message,
  );
  const noHash = throws(() => resolvePatchPaths([{ path: patchA }]));
  assert(
    noHash && noHash.message.includes("64-hex"),
    "a patch pin without a SHA-256 is rejected",
    noHash?.message,
  );

  assertPatchResult({ status: 0, stdout: "", stderr: "" }, "ok.patch");
  const rejected = throws(() =>
    assertPatchResult(
      { status: 1, stdout: "patching file x\n", stderr: "Hunk #1 FAILED at 1." },
      "mpv-fix-missing-objc.patch",
    ),
  );
  assert(
    rejected &&
      rejected.message.includes("mpv-fix-missing-objc.patch") &&
      rejected.message.includes("Hunk #1 FAILED"),
    "a rejected patch fails and includes the patch name and diagnostics",
    rejected?.message,
  );
  assert(
    throws(() => assertPatchResult({ error: new Error("spawn patch ENOENT") }, "x.patch")) !== null,
    "a patch that cannot start fails instead of being skipped",
  );
}

// --- meson option validation ------------------------------------------------
{
  const declared = parseMesonOptionNames(MPV_036_OPTIONS);
  assert(declared.has("libmpv") && declared.has("videotoolbox-gl"), "the 0.36.0 option table parses");
  assert(
    findUnknownMesonOptions(MPV_MESON_OPTIONS, declared).length === 0,
    "every configured -D option is declared by mpv 0.36.0",
    JSON.stringify(findUnknownMesonOptions(MPV_MESON_OPTIONS, declared)),
  );
  assert(
    equal(findUnknownMesonOptions(["-Dvideotoolbox-pl=disabled"], declared), ["videotoolbox-pl"]),
    "the nonexistent videotoolbox-pl option is rejected",
  );
  assert(optionName("-Dgl=disabled") === "gl", "optionName extracts the -D name");
  assert(throws(() => optionName("--enable-gl")) !== null, "non -D options are rejected by the validator");
  assert(throws(() => parseMesonOptionNames("")) !== null, "an empty option table is rejected");
}

// --- configure warnings -----------------------------------------------------
{
  assert(
    equal(findConfigureWarnings("Enabled decoders:\n  mp3\n"), []),
    "a clean configure log has no warnings",
  );
  const warnings = findConfigureWarnings(
    "WARNING: --enable-decoder=mp4: did not match anything\nEnabled decoders:\n",
  );
  assert(
    warnings.length === 1 && warnings[0].includes("did not match anything"),
    "a component that matched nothing is surfaced as a warning",
    JSON.stringify(warnings),
  );
}

// --- pkg-config rewrite -----------------------------------------------------
{
  // FFmpeg 6.0's real .pc shape: libdir/includedir are absolute, not
  // ${prefix}-relative. Rewriting only prefix= leaves -I/usr/local/include.
  const pc =
    "prefix=/usr/local\nexec_prefix=${prefix}\nlibdir=/usr/local/lib\n" +
    "includedir=/usr/local/include\n\nName: libavcodec\n";
  const staging = "/tmp/build/ffmpeg-dest/usr/local";
  const rewritten = rewritePkgConfigPrefix(pc, staging);
  assert(
    rewritten.startsWith(`prefix=${staging}\n`) &&
      rewritten.includes(`libdir=${staging}/lib\n`) &&
      rewritten.includes(`includedir=${staging}/include\n`),
    "prefix, libdir and includedir all point at the staging tree",
    rewritten,
  );
  assert(
    rewritten.includes("exec_prefix=${prefix}"),
    "derived fields stay relative",
  );
  const relative = "prefix=/usr/local\nexec_prefix=${prefix}\nlibdir=${prefix}/lib\n";
  assert(
    rewritePkgConfigPrefix(relative, staging).includes("libdir=${prefix}/lib"),
    "${prefix}-relative fields are left untouched",
  );
  assert(
    rewritePkgConfigPrefix("prefix=/usr/local\nlibdir=/opt/homebrew/lib\n", staging).includes(
      "libdir=/opt/homebrew/lib",
    ),
    "absolute paths outside the configure prefix are not rewritten",
  );
  assert(rewritePkgConfigPrefix(pc, "/x") === rewritePkgConfigPrefix(pc, "/x"), "the rewrite is deterministic");
}

// --- otool parsing and dependency classification ----------------------------
{
  const OTOOL = `libmpv.dylib:
\t@rpath/libmpv.dylib (compatibility version 2.0.0, current version 2.2.0)
\t@loader_path/libavcodec.dylib (compatibility version 60.0.0, current version 60.3.100)
\t/System/Library/Frameworks/CoreAudio.framework/Versions/A/CoreAudio (compatibility version 1.0.0, current version 1.0.0)
\t@loader_path/libass.9.dylib (compatibility version 9.0.0, current version 9.2.0)
`;
  const refs = parseOtoolLibraries(OTOOL);
  assert(
    equal(refs, [
      "@rpath/libmpv.dylib",
      "@loader_path/libavcodec.dylib",
      "/System/Library/Frameworks/CoreAudio.framework/Versions/A/CoreAudio",
      "@loader_path/libass.9.dylib",
    ]),
    "otool -L parses every reference including the self ID",
    JSON.stringify(refs),
  );
}

{
  assert(normalizeDylibName("libavcodec.60.dylib") === "libavcodec.dylib", "FFmpeg major name normalizes");
  assert(normalizeDylibName("libavcodec.60.31.102.dylib") === "libavcodec.dylib", "FFmpeg full version normalizes");
  assert(normalizeDylibName("libmpv.2.dylib") === "libmpv.dylib", "mpv version normalizes");
  assert(normalizeDylibName("libass.9.dylib") === "libass.9.dylib", "auxiliary names keep their major version");
}

{
  const homebrew = classifyDependencyPath("/opt/homebrew/opt/libass/lib/libass.9.dylib", {
    homebrewPrefix: "/opt/homebrew",
    stagingPrefixes: ["/tmp/build/ffmpeg-dest/usr/local/lib"],
  });
  assert(homebrew.kind === "homebrew" && homebrew.target === "libass.9.dylib", "Homebrew paths classify as homebrew");

  const system = classifyDependencyPath("/System/Library/Frameworks/CoreAudio.framework/Versions/A/CoreAudio", {
    homebrewPrefix: "/opt/homebrew",
    stagingPrefixes: [],
  });
  assert(system.kind === "system", "system frameworks classify as system");

  const staging = classifyDependencyPath("/tmp/build/ffmpeg-dest/usr/local/lib/libavcodec.60.dylib", {
    homebrewPrefix: "/opt/homebrew",
    stagingPrefixes: ["/tmp/build/ffmpeg-dest/usr/local/lib"],
  });
  assert(staging.kind === "staging" && staging.target === "libavcodec.dylib", "staged FFmpeg normalizes to the release name");

  const local = classifyDependencyPath("@loader_path/libavcodec.dylib", {
    homebrewPrefix: "/opt/homebrew",
    stagingPrefixes: [],
  });
  assert(local.kind === "local" && local.target === "libavcodec.dylib", "loader-path references are local");

  const unknown = classifyDependencyPath("/private/tmp/random/libfoo.dylib", {
    homebrewPrefix: "/opt/homebrew",
    stagingPrefixes: [],
  });
  assert(unknown.kind === "unknown", "unrecognized absolute paths fail closed");
}

// --- install-name plan ------------------------------------------------------
{
  const files = ["libass.9.dylib", "libavcodec.dylib", "libmpv.dylib"];
  const plan = planInstallNameChanges({
    files,
    referencesByFile: {
      "libmpv.dylib": [
        "@rpath/libmpv.dylib",
        "@rpath/libavcodec.60.dylib",
        "/opt/homebrew/opt/libass/lib/libass.9.dylib",
        "/System/Library/Frameworks/CoreAudio.framework/Versions/A/CoreAudio",
        "@rpath/libmissing.dylib",
      ],
      "libavcodec.dylib": ["@rpath/libavcodec.dylib"],
      "libass.9.dylib": ["@loader_path/libass.9.dylib"],
    },
  });
  assert(
    equal(plan.ids.map((entry) => entry.installName).sort(), [
      "@rpath/libass.9.dylib",
      "@rpath/libavcodec.dylib",
      "@rpath/libmpv.dylib",
    ]),
    "every candidate gets an @rpath identity",
    JSON.stringify(plan.ids),
  );
  assert(
    plan.changes.some(
      (entry) =>
        entry.file === "libmpv.dylib" &&
        entry.from === "@rpath/libavcodec.60.dylib" &&
        entry.to === "@loader_path/libavcodec.dylib",
    ),
    "a versioned FFmpeg reference is rewritten to the unversioned sibling",
    JSON.stringify(plan.changes),
  );
  assert(
    plan.changes.some(
      (entry) =>
        entry.file === "libmpv.dylib" &&
        entry.from === "/opt/homebrew/opt/libass/lib/libass.9.dylib" &&
        entry.to === "@loader_path/libass.9.dylib",
    ),
    "a Homebrew reference is rewritten to the bundled sibling",
  );
  assert(
    !plan.changes.some((entry) => entry.from === "@rpath/libmpv.dylib"),
    "the LC_ID_DYLIB self reference is left to install_name_tool -id",
  );
  assert(
    plan.systemReferences.length === 1 && plan.systemReferences[0].reference.includes("CoreAudio"),
    "system references are preserved, not rewritten",
    JSON.stringify(plan.systemReferences),
  );
  assert(
    plan.unknownReferences.length === 1 && plan.unknownReferences[0].reference === "@rpath/libmissing.dylib",
    "unresolved references are reported for the caller to reject",
    JSON.stringify(plan.unknownReferences),
  );
}

// --- real vendor plan (macOS only) ------------------------------------------
if (process.platform === "darwin") {
  const vendor = resolve("apps/desktop/src-tauri/vendor/libmpv/macos");
  const output = spawnSync("otool", ["-L", resolve(vendor, "libmpv.dylib")], {
    encoding: "utf8",
  });
  if (output.status !== 0) {
    assert(false, "the current vendor libmpv.dylib can be inspected", output.stderr);
  } else {
    const refs = parseOtoolLibraries(output.stdout);
    const targets = refs.map((ref) => normalizeDylibName(ref.slice(ref.lastIndexOf("/") + 1)));
    assert(
      targets.includes("libass.9.dylib") && targets.includes("libavcodec.dylib"),
      "the real vendor libmpv references libass.9.dylib and libavcodec.dylib",
      JSON.stringify(targets),
    );
  }
}

// --- NOTICE generated block -------------------------------------------------
{
  const auxiliary = [
    { component: "libass", version: "0.17.5_1", files: ["libass.9.dylib"] },
    { component: "FreeType", version: "2.14.3", files: ["libfreetype.6.dylib"] },
  ];
  const block = renderNoticeBlock(auxiliary);
  assert(block.startsWith("<!-- BEGIN GENERATED AUXILIARY VERSIONS -->"), "the notice block starts with its sentinel");
  assert(block.includes("| FreeType | Homebrew 2.14.3 |"), "auxiliary versions render as table rows");

  const prose = "# Notice\n\nHand-written legal text.\n";
  const merged = mergeNoticeBlock(prose, block);
  assert(merged.includes("Hand-written legal text."), "prose outside the block is preserved");
  assert(mergeNoticeBlock(merged, block) === merged, "merging the same block twice is byte-identical");
  const replaced = mergeNoticeBlock(
    merged.replace("0.17.5_1", "0.17.6"),
    renderNoticeBlock([{ component: "libass", version: "0.17.7", files: ["libass.9.dylib"] }]),
  );
  assert(replaced.includes("0.17.7") && !replaced.includes("0.17.6"), "an existing block is replaced, not duplicated");

  // A partially-written / duplicated generated block must fail loudly instead
  // of leaving a stale table and appending a second BEGIN block.
  const begin = "<!-- BEGIN GENERATED AUXILIARY VERSIONS -->";
  const end = "<!-- END GENERATED AUXILIARY VERSIONS -->";
  const malformed = [
    ["missing END", `# Notice\n${begin}\n| stale |\n`],
    ["missing BEGIN", `# Notice\n| stale |\n${end}\n`],
    ["two BEGIN", `# Notice\n${block}\n${block}\n`],
    ["END before BEGIN", `${end}\n${block}\n`],
    ["two END", `${begin}\n| stale |\n${end}\n${end}\n`],
  ];
  for (const [label, text] of malformed) {
    const error = throws(() => mergeNoticeBlock(text, block));
    assert(
      error && /malformed/.test(error.message),
      `a NOTICE with ${label} is rejected instead of producing a duplicate block`,
      error?.message,
    );
  }
  assert(
    mergeNoticeBlock(prose, block) === merged,
    "a normal replacement stays byte-identical across runs (idempotent)",
  );
}

// --- manifest record writer -------------------------------------------------
{
  const manifest = {
    schemaVersion: 1,
    architectures: ["arm64"],
    sourceUrl: "https://ffmpeg.org/releases/ffmpeg-6.0.tar.xz",
    assetSha256: "a".repeat(64),
    files: { "libold.dylib": "old" },
    fileArchitectures: { "libold.dylib": ["arm64"] },
  };
  const builds = {
    ffmpeg: {
      version: "6.0",
      configure: [
        "--disable-everything",
        "--enable-filter=aformat,alimiter,equalizer,extrastereo,volume,aresample",
      ],
      deploymentTarget: "27.0",
      toolchain: "clang",
    },
    mpv: {
      version: "0.36.0",
      meson: ["-Dlibmpv=true"],
      patches: ["scripts/release/patches/a.patch"],
      hardDependencies: ["libass.9.dylib", "libswscale.dylib"],
    },
    auxiliary: [{ component: "libass", version: "0.17.5_1", files: ["libass.9.dylib"] }],
  };
  const input = {
    files: ["libmpv.dylib", "libavcodec.dylib"],
    fileHashes: { "libmpv.dylib": "m", "libavcodec.dylib": "c" },
    builds,
    buildHost: "macOS 27.2 arm64",
  };
  const next = applyBuildRecords(manifest, input);
  assert(
    equal(Object.keys(next.files), ["libavcodec.dylib", "libmpv.dylib"]),
    "the file map is rebuilt deterministically in sorted order",
    JSON.stringify(next.files),
  );
  assert(
    next.sourceUrl === manifest.sourceUrl && next.assetSha256 === manifest.assetSha256,
    "source pins are not rewritten by the record writer",
  );
  assert(next.buildScript === "scripts/release/build-macos-libmpv.mjs", "the build script path is recorded");
  assert(next.builds.ffmpeg.version === "6.0" && next.builds.mpv.version === "0.36.0", "build records are attached");
  assert(
    next.ffmpegBuild.includes("aresample") && next.ffmpegBuild.includes("deployment target 27.0"),
    "the FFmpeg summary is derived from the recorded configure and target",
    next.ffmpegBuild,
  );
  assert(
    next.mpvBuild.includes("a.patch") &&
      next.mpvBuild.includes("libass.9.dylib") &&
      next.mpvBuild.includes("libswscale.dylib"),
    "the mpv summary is derived from the recorded patches and hard dependencies",
    next.mpvBuild,
  );
  assert(
    summarizeFfmpegBuild(builds) === next.ffmpegBuild && summarizeMpvBuild(builds) === next.mpvBuild,
    "build summaries are pure functions of the recorded builds",
  );
  const twice = applyBuildRecords(next, input);
  assert(JSON.stringify(twice) === JSON.stringify(next), "applying the same build record twice is idempotent");
}

// --- minos parsing ----------------------------------------------------------
{
  const otool = "      cmd LC_BUILD_VERSION\n  cmdsize 32\n platform 1\n    minos 27.0\n      sdk 27.2\n";
  assert(parseMinos(otool) === "27.0", "LC_BUILD_VERSION minos is parsed");
  assert(parseMinos("no build version") === null, "a missing minos is reported as null");
  assert(compareDottedVersions("27.0", "27.2") < 0 && compareDottedVersions("27.2", "27.0") > 0, "versions compare numerically");
  assert(
    deploymentTargetViolation("libavcodec.dylib", "27.2") !== null,
    "a built dylib newer than the pin is rejected",
  );
  assert(
    deploymentTargetViolation("libavcodec.dylib", "26.0") !== null,
    "a built dylib older than the pin is rejected",
  );
  assert(
    deploymentTargetViolation("libgraphite2.3.dylib", "26.0") === null,
    "an auxiliary dylib built for an older target is allowed",
  );
  assert(
    deploymentTargetViolation("libgraphite2.3.dylib", "27.2") !== null,
    "an auxiliary dylib newer than the pin is rejected",
  );

  // The minos field is anchored to a whole line inside an LC_BUILD_VERSION
  // command. Path text, unrelated commands and trailing dots must not leak in.
  const buildVersion = (minos) =>
    `Load command 9\n      cmd LC_BUILD_VERSION\n  cmdsize 32\n platform 1\n    minos ${minos}\n      sdk 27.2\n`;
  assert(
    parseMinos(`/tmp/build/minos 99.0/ffmpeg:\n${buildVersion("27.0")}`) === "27.0",
    "a 'minos 99.0' fragment in the inspected path is ignored",
  );
  assert(
    parseMinos("Load command 3\n      cmd LC_VERSION_MIN_MACOSX\n    version 27.0\n") === null,
    "LC_VERSION_MIN_MACOSX's version is not mistaken for a build-version minos",
  );
  assert(parseMinos(buildVersion("27.")) === null, "a minos with a trailing dot is rejected");
  assert(parseMinos(buildVersion("27.0.1")) === "27.0.1", "a multi-component minos is accepted");
  assert(
    throws(() =>
      parseMinos(`${buildVersion("27.0")}\nLoad command 10\n${buildVersion("26.0")}`),
    ) !== null,
    "two LC_BUILD_VERSION blocks with conflicting minos values fail",
  );
  assert(
    parseMinos(`${buildVersion("27.0")}\nLoad command 10\n${buildVersion("27.0")}`) === "27.0",
    "two LC_BUILD_VERSION blocks that agree resolve to the single value",
  );
}

// --- FFmpeg option invariants ----------------------------------------------
{
  const args = FFMPEG_CONFIGURE_ARGS.join(" ");
  assert(FFMPEG_CONFIGURE_ARGS.includes("--disable-everything"), "FFmpeg is configured with --disable-everything");
  assert(args.includes("--enable-demuxer=mp3,flac,ape,mov,ogg,wav"), "all seven guaranteed containers are enabled");
  assert(
    args.includes("--enable-filter=aformat,alimiter,equalizer,extrastereo,volume"),
    "the five audio-effect filters are enabled",
  );
  assert(!args.includes("--enable-decoder=h264"), "video decoders are not enabled");
  assert(
    args.includes(`-mmacosx-version-min=${MACOSX_DEPLOYMENT_TARGET}`),
    "both compile and link flags pin the macOS deployment target",
  );
  assert(MACOSX_DEPLOYMENT_TARGET === "27.0", "the deployment target stays at the 27.0 vendor baseline");
  assert(
    MPV_HARD_DEPENDENCIES.includes("libass.9.dylib") && MPV_HARD_DEPENDENCIES.includes("libswscale.dylib"),
    "libmpv's hard dependencies are declared",
  );
}

// --- destructive scratch paths ---------------------------------------------
{
  const repository = resolve("/tmp/echo-test-repo");
  const safe = resolve("/tmp/echo-test-work");
  assert(assertSafeScratchDir(safe, repository) === safe, "a scratch directory outside the repository is allowed");
  assert(
    throws(() => assertSafeScratchDir(repository, repository)) !== null,
    "the repository root itself is refused as scratch space",
  );
  assert(
    throws(() => assertSafeScratchDir(resolve(repository, "target", "work"), repository)) !== null,
    "a scratch directory inside the repository is refused",
  );
  assert(
    throws(() => assertSafeScratchDir(resolve("/tmp"), repository)) !== null,
    "a top-level directory is refused as scratch space",
  );
  assert(
    throws(() => assertSafeScratchDir(resolve("/"), repository)) !== null,
    "the filesystem root is refused as scratch space",
  );
  // The builder recursively deletes --work / --out. Home itself and any
  // first-level child (Desktop / Documents / Downloads / ...) hold the user's
  // real data, so all of them must be refused before any rmSync can run.
  const home = homedir();
  assert(
    throws(() => assertSafeScratchDir(home, repository)) !== null,
    "the user's home directory is refused as scratch space",
  );
  for (const child of ["Desktop", "Documents", "Downloads"]) {
    assert(
      throws(() => assertSafeScratchDir(resolve(home, child), repository)) !== null,
      `${home}/${child}, a first-level child of home, is refused as scratch space`,
    );
  }
  assert(
    assertSafeScratchDir(resolve(home, "work", "echo-build"), repository) === resolve(home, "work", "echo-build"),
    "a scratch directory nested more than one level under home is allowed",
  );
  assert(
    assertSafeScratchDir(resolve("/tmp", "echo-test-work"), repository) === resolve("/tmp", "echo-test-work"),
    "a /tmp scratch directory is allowed",
  );
}

// --- Homebrew version provenance -------------------------------------------
{
  assert(
    homebrewVersionFromSource(
      "/opt/homebrew/Cellar/libass/0.17.4/lib/libass.9.dylib",
      "libass",
    ) === "0.17.4",
    "the auxiliary version comes from the exact Cellar path that entered the closure",
  );
  assert(
    throws(() => homebrewVersionFromSource("/usr/local/lib/libass.9.dylib", "libass")) !== null,
    "an auxiliary dylib outside a Homebrew Cellar fails instead of recording a guessed version",
  );
}

// --- vendor backup / rollback ----------------------------------------------
{
  const vendor = mkdtempSync(resolve(tmpdir(), "echo-vendor-test-"));
  mkdirSync(vendor, { recursive: true });
  writeFileSync(resolve(vendor, "libmpv.dylib"), "old-mpv");
  writeFileSync(resolve(vendor, "libass.9.dylib"), "old-ass");
  writeFileSync(resolve(vendor, "manifest.json"), '{"old":true}\n');
  writeFileSync(resolve(vendor, "NOTICE.md"), "old notice\n");
  const backup = backupVendorTree(vendor);
  writeFileSync(resolve(vendor, "libmpv.dylib"), "new-mpv");
  writeFileSync(resolve(vendor, "libnew.dylib"), "new-extra");
  writeFileSync(resolve(vendor, "manifest.json"), '{"new":true}\n');
  restoreVendorTree(vendor, backup);
  assert(
    readFileSync(resolve(vendor, "libmpv.dylib"), "utf8") === "old-mpv" &&
      readFileSync(resolve(vendor, "libass.9.dylib"), "utf8") === "old-ass" &&
      readFileSync(resolve(vendor, "manifest.json"), "utf8") === '{"old":true}\n' &&
      readFileSync(resolve(vendor, "NOTICE.md"), "utf8") === "old notice\n",
    "rollback restores the previous dylibs, manifest and notice",
  );
  assert(
    !readdirSync(vendor).includes("libnew.dylib"),
    "rollback removes dylibs that were added by the failed write",
  );
}

process.stdout.write(
  failures === 0
    ? "\nbuild-macos-libmpv self-test: all ok\n"
    : `\nbuild-macos-libmpv self-test: ${failures} failure(s)\n`,
);
process.exit(failures === 0 ? 0 : 1);
