#!/usr/bin/env node
// Injection self-test for the runtime component probe's judge functions.
//
// Every forbidden/required rule gets a constructed violation, so a future
// refactor cannot turn an assertion into a silent no-op. The legal fixture
// includes all four unconditional graph endpoints: buffer/buffersink are video
// graph endpoints and must not be mistaken for video processing filters.

import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import {
  assertConfigureMatches,
  assertForbiddenLinkage,
  assertMinimalComponents,
  assertProbeLoadedFrom,
  classifyComponents,
  parseConfigureString,
  parsePadTypes,
  parseProbeOutput,
} from "./ffmpeg-component-probe.mjs";
// Importing the build script here is side-effect free (it has a main guard) and
// does not create a cycle: build -> probe is one-way, and this test imports both.
// Deriving the fixture from FFMPEG_CONFIGURE_ARGS keeps the embedded configure
// string in lockstep with the real 25-token build instead of a stale 7.1-era copy.
import { FFMPEG_CONFIGURE_ARGS } from "./build-macos-libmpv.mjs";

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

// Rebuild the configure string exactly the way FFmpeg 6.0's
// avcodec_configuration() embeds it: an --opt=value is single-quoted when its
// value contains whitespace, a comma or another `=`. This is byte-identical to
// the real 25-token embedded string (including the --extra-cflags/-ldflags
// min-version suffixes and the --enable-decoder=... token), not the stale
// 7.1-era fixture that lacked them.
function embedConfigureArg(token) {
  const eq = token.indexOf("=");
  if (eq === -1) return token;
  const value = token.slice(eq + 1);
  return /[\s,=]/.test(value) ? `${token.slice(0, eq + 1)}'${value}'` : token;
}
const CONFIG = FFMPEG_CONFIGURE_ARGS.map(embedConfigureArg).join(" ");
const CONFIG_ARGS = [...FFMPEG_CONFIGURE_ARGS];

function legalOutput({
  decoders = [
    ["mp3", 1],
    ["mp3float", 1],
    ["flac", 1],
    ["ape", 1],
    ["aac", 1],
    ["aac_latm", 1],
    ["alac", 1],
    ["vorbis", 1],
    ["opus", 1],
    ["pcm_s16le", 1],
    ["pcm_s24le", 1],
    ["pcm_f32le", 1],
  ],
  encoders = [],
  filters = [
    ["aformat", "1", "1"],
    ["alimiter", "1", "1"],
    ["equalizer", "1", "1"],
    ["extrastereo", "1", "1"],
    ["volume", "1", "1"],
    ["aresample", "1", "1"],
    ["abuffer", "-", "1"],
    ["abuffersink", "1", "-"],
    ["buffer", "-", "0"],
    ["buffersink", "0", "-"],
  ],
  demuxers = ["mp3", "flac", "ape", "mov,mp4,m4a,3gp,3g2,mj2", "ogg", "wav"],
  muxers = [],
  protocolsIn = ["file"],
  protocolsOut = ["file"],
  config = CONFIG,
  version = "6.0",
  mpv = "131072",
  omitCounts = false,
  countsOverride,
} = {}) {
  const lines = [
    `version\t${version}`,
    `config\t${config}`,
    `mpv\t${mpv}`,
    "loaded\tavcodec_configuration\t/vendor/libavcodec.dylib",
    "loaded\tavfilter_filter_pad_count\t/vendor/libavfilter.dylib",
    "loaded\tav_demuxer_iterate\t/vendor/libavformat.dylib",
    "loaded\tmpv_client_api_version\t/vendor/libmpv.dylib",
  ];
  for (const [name, type] of decoders) lines.push(`decoder\t${name}\t${type}`);
  for (const [name, type] of encoders) lines.push(`encoder\t${name}\t${type}`);
  for (const [name, inputs, outputs] of filters) lines.push(`filter\t${name}\t${inputs}\t${outputs}`);
  for (const name of demuxers) lines.push(`demuxer\t${name}`);
  for (const name of muxers) lines.push(`muxer\t${name}`);
  for (const name of protocolsIn) lines.push(`protocol_in\t${name}`);
  for (const name of protocolsOut) lines.push(`protocol_out\t${name}`);
  if (!omitCounts) {
    const counts = countsOverride ?? {
      filters: filters.length,
      decoders: decoders.length,
      encoders: encoders.length,
      demuxers: demuxers.length,
      muxers: muxers.length,
      protocols_in: protocolsIn.length,
      protocols_out: protocolsOut.length,
    };
    lines.push(
      `counts\tfilters=${counts.filters} decoders=${counts.decoders} encoders=${counts.encoders} ` +
        `demuxers=${counts.demuxers} muxers=${counts.muxers} ` +
        `protocols_in=${counts.protocols_in} protocols_out=${counts.protocols_out}`,
    );
  }
  return lines.join("\n");
}

// --- legal build ------------------------------------------------------------
{
  const parsed = parseProbeOutput(legalOutput());
  const verdict = assertMinimalComponents(parsed, {
    expectedVersion: "6.0",
    expectedConfigure: CONFIG_ARGS,
  });
  assert(
    verdict.counts.decoders === 12 &&
      verdict.counts.filters === 10 &&
      verdict.counts.demuxers === 6 &&
      verdict.counts.encoders === 0 &&
      verdict.counts.muxers === 0 &&
      verdict.counts.protocolsIn === 1 &&
      verdict.counts.protocolsOut === 1,
    "the legal minimal component set passes with expected counts and the real configure string",
    JSON.stringify(verdict.counts),
  );
  assert(
    parsed.filters.some((entry) => entry.name === "buffer") &&
      parsed.filters.some((entry) => entry.name === "buffersink"),
    "buffer/buffersink are parsed as allowed graph endpoints",
  );
  assert(
    parsePadTypes("-").length === 0 && parsePadTypes("1,2").join(",") === "1,2",
    "pad type lists parse",
  );
  assert(
    throws(() => parsePadTypes(" ")) !== null && throws(() => parsePadTypes("1,,2")) !== null,
    "empty and malformed pad fields are rejected instead of becoming pad type 0",
  );
}

// --- forbidden components ---------------------------------------------------
{
  const parsed = parseProbeOutput(legalOutput({ decoders: [["h264", 0]] }));
  const verdict = classifyComponents(parsed);
  assert(
    verdict.violations.some((entry) => entry.includes("h264") && entry.includes("non-audio")),
    "a video decoder is rejected and named",
    JSON.stringify(verdict.violations),
  );
}

{
  const parsed = parseProbeOutput(legalOutput({ decoders: [...legalOutputDecoders(), ["theora", 1]] }));
  const verdict = classifyComponents(parsed);
  assert(
    verdict.violations.some((entry) => entry.includes("unexpected decoder registered: theora")),
    "an unexpected decoder is rejected even when its media type says audio",
    JSON.stringify(verdict.violations),
  );
}

{
  const parsed = parseProbeOutput(
    legalOutput({
      filters: [
        ["aformat", "1", "1"],
        ["alimiter", "1", "1"],
        ["equalizer", "1", "1"],
        ["extrastereo", "1", "1"],
        ["volume", "1", "1"],
        ["aresample", "1", "1"],
        ["abuffer", "-", "1"],
        ["abuffersink", "1", "-"],
        ["buffer", "-", "0"],
        ["buffersink", "0", "-"],
        ["scale", "0", "0"],
      ],
    }),
  );
  const verdict = classifyComponents(parsed);
  assert(
    verdict.violations.some((entry) => entry.includes("unexpected filter registered: scale")),
    "a video processing filter is rejected and named",
    JSON.stringify(verdict.violations),
  );
}

{
  const parsed = parseProbeOutput(
    legalOutput({ demuxers: ["matroska", "mp3", "flac", "ape", "mov,mp4", "ogg", "wav"] }),
  );
  const verdict = classifyComponents(parsed);
  assert(
    verdict.violations.some((entry) => entry.includes("unexpected demuxer registered: matroska")),
    "a video container demuxer is rejected and named",
    JSON.stringify(verdict.violations),
  );
}

{
  const parsed = parseProbeOutput(legalOutput({ encoders: [["aac", 1]] }));
  const verdict = classifyComponents(parsed);
  assert(
    verdict.violations.some((entry) => entry.includes("unexpected encoder registered: aac")),
    "a registered encoder is rejected and named",
    JSON.stringify(verdict.violations),
  );
}

{
  const parsed = parseProbeOutput(legalOutput({ muxers: [["matroska"]] }));
  const verdict = classifyComponents(parsed);
  assert(
    verdict.violations.some((entry) => entry.includes("unexpected muxer registered: matroska")),
    "a registered muxer is rejected and named",
    JSON.stringify(verdict.violations),
  );
}

// --- missing required components -------------------------------------------
{
  const parsed = parseProbeOutput(legalOutput({ demuxers: ["mp3", "flac", "mov,mp4", "ogg", "wav"] }));
  const verdict = classifyComponents(parsed);
  assert(
    verdict.violations.includes("required demuxer missing: ape"),
    "a missing APE demuxer is rejected and named",
    JSON.stringify(verdict.violations),
  );
}

{
  const parsed = parseProbeOutput(
    legalOutput({
      filters: [
        ["aformat", "1", "1"],
        ["alimiter", "1", "1"],
        ["equalizer", "1", "1"],
        ["extrastereo", "1", "1"],
        ["aresample", "1", "1"],
      ],
    }),
  );
  const verdict = classifyComponents(parsed);
  assert(
    verdict.violations.includes("required filter missing: volume") &&
      verdict.violations.includes("required graph endpoint missing: buffer"),
    "missing audio filters and missing graph endpoints are both named",
    JSON.stringify(verdict.violations),
  );
}

{
  const parsed = parseProbeOutput(legalOutput({ decoders: [["mp3", 1], ["pcm_s16le", 1]] }));
  const verdict = classifyComponents(parsed);
  assert(
    verdict.violations.some((entry) => entry.includes("required decoder missing: ape")),
    "missing APE decoder is rejected and named",
    JSON.stringify(verdict.violations),
  );
}

// --- pad contracts ----------------------------------------------------------
{
  const parsed = parseProbeOutput(
    legalOutput({
      filters: [
        ["aformat", "1", "0"],
        ["alimiter", "1", "1"],
        ["equalizer", "1", "1"],
        ["extrastereo", "1", "1"],
        ["volume", "1", "1"],
        ["aresample", "1", "1"],
        ["abuffer", "-", "1"],
        ["abuffersink", "1", "-"],
        ["buffer", "-", "0"],
        ["buffersink", "0", "-"],
      ],
    }),
  );
  const verdict = classifyComponents(parsed);
  assert(
    verdict.violations.some((entry) => entry.includes("filter aformat has unexpected output pads [0]")),
    "an audio filter with a video pad is rejected",
    JSON.stringify(verdict.violations),
  );
}

{
  const parsed = parseProbeOutput(
    legalOutput({
      filters: [
        ["aformat", "1", "1"],
        ["alimiter", "1", "1"],
        ["equalizer", "1", "1"],
        ["extrastereo", "1", "1"],
        ["volume", "1", "1"],
        ["aresample", "1", "1"],
        ["abuffer", "1", "1"],
        ["abuffersink", "1", "-"],
        ["buffer", "-", "0"],
        ["buffersink", "0", "-"],
      ],
    }),
  );
  const verdict = classifyComponents(parsed);
  assert(
    verdict.violations.some((entry) => entry.includes("filter abuffer has unexpected input pads [1]")),
    "a source endpoint with an unexpected input pad is rejected",
    JSON.stringify(verdict.violations),
  );
}

{
  const error = throws(() =>
    parseProbeOutput(
      legalOutput({
        filters: [
          ["aformat", "", "1"],
          ["alimiter", "1", "1"],
          ["equalizer", "1", "1"],
          ["extrastereo", "1", "1"],
          ["volume", "1", "1"],
          ["aresample", "1", "1"],
          ["abuffer", "-", "1"],
          ["abuffersink", "1", "-"],
          ["buffer", "-", "0"],
          ["buffersink", "0", "-"],
        ],
      }),
    ),
  );
  assert(
    error !== null && error.message.includes("no pad type field"),
    "an empty pad field is rejected by the parser before classification",
    error?.message,
  );
}

// --- protocols and configure string ----------------------------------------
{
  const parsed = parseProbeOutput(legalOutput({ protocolsIn: ["file", "https"] }));
  const verdict = classifyComponents(parsed);
  assert(
    verdict.violations.some((entry) => entry.includes("input protocols are [file, https]")),
    "a network input protocol is rejected and the set is printed",
    JSON.stringify(verdict.violations),
  );
}

{
  const parsed = parseProbeOutput(legalOutput({ protocolsOut: ["file", "https"] }));
  const verdict = classifyComponents(parsed);
  assert(
    verdict.violations.some((entry) => entry.includes("output protocols are [file, https]")),
    "a network output protocol is rejected and the set is printed",
    JSON.stringify(verdict.violations),
  );
}

{
  const parsed = parseProbeOutput(legalOutput({ config: CONFIG.replace("--disable-everything ", "") }));
  const verdict = classifyComponents(parsed);
  assert(
    verdict.violations.some((entry) => entry.includes("lacks --disable-everything")),
    "a configure string without --disable-everything is rejected",
    JSON.stringify(verdict.violations),
  );
}

{
  const parsed = parseProbeOutput(legalOutput({ config: `${CONFIG} --enable-videotoolbox` }));
  const verdict = classifyComponents(parsed);
  assert(
    verdict.violations.some((entry) => entry.includes("--enable-videotoolbox")),
    "a video toolbox enable flag in the embedded config is rejected",
    JSON.stringify(verdict.violations),
  );
}

{
  const tokens = parseConfigureString(
    "--prefix=/usr/local --enable-demuxer='mp3,flac' --extra-ldflags='-Wl,-dead_strip_dylibs -mmacosx-version-min=27.0'",
  );
  assert(
    JSON.stringify(tokens) ===
      JSON.stringify([
        "--prefix=/usr/local",
        "--enable-demuxer=mp3,flac",
        "--extra-ldflags=-Wl,-dead_strip_dylibs -mmacosx-version-min=27.0",
      ]),
    "quoted configure tokens are normalized before comparison",
    JSON.stringify(tokens),
  );
  assert(
    throws(() => assertConfigureMatches(CONFIG, ["--prefix=/usr/local"]))?.message.includes("token 1"),
    "configure drift names the first differing token",
  );
}

// --- forbidden configure options (token-boundary bypasses) ------------------
// The LGPL-only build must reject every GPL/nonfree/video/TLS enable switch,
// including letter/digit-suffixed variants that a plain \b let slip through.
{
  for (const option of [
    "--enable-gpl",
    "--enable-gpl3",
    "--enable-gplfoo",
    "--enable-version3",
    "--enable-nonfree",
    "--enable-videotoolbox",
    "--enable-openssl",
  ]) {
    const parsed = parseProbeOutput(legalOutput({ config: `${CONFIG} ${option}` }));
    const verdict = classifyComponents(parsed);
    assert(
      verdict.violations.some((entry) => entry.includes(option)),
      `forbidden configure option ${option} is rejected and named`,
      JSON.stringify(verdict.violations),
    );
  }
  const legal = classifyComponents(parseProbeOutput(legalOutput()), {
    expectedConfigure: CONFIG_ARGS,
  });
  assert(
    legal.violations.filter((entry) => entry.includes("configure")).length === 0,
    "the real 25-token configure string has zero configure violations",
    JSON.stringify(legal.violations),
  );
}

// --- real configure string compared token-by-token -------------------------
// The old test only ever compared against a synthetic ["--prefix=/usr/local"],
// so a drifted build script or manifest would never fail here.
{
  const parsed = parseProbeOutput(legalOutput());
  assert(
    throws(() => assertMinimalComponents(parsed, { expectedConfigure: CONFIG_ARGS })) === null,
    "the real 25-token configure string matches FFMPEG_CONFIGURE_ARGS exactly",
  );
  assert(
    parseConfigureString(CONFIG).length === 25,
    "the legal fixture really is the 25-token configure string",
    String(parseConfigureString(CONFIG).length),
  );
  for (const dropped of ["--disable-network", "--disable-autodetect", "--disable-everything"]) {
    const drifted = CONFIG_ARGS.filter((token) => token !== dropped);
    const error = throws(() =>
      assertMinimalComponents(parsed, { expectedConfigure: drifted }),
    );
    assert(
      error !== null,
      `dropping ${dropped} from the expected configure string fails`,
      error?.message,
    );
  }
  const extraToken = throws(() =>
    assertMinimalComponents(parsed, { expectedConfigure: [...CONFIG_ARGS, "--enable-libx264"] }),
  );
  assert(extraToken !== null, "adding an unexpected configure token fails", extraToken?.message);
}

// --- explicit --disable-* invariants (item 10) -----------------------------
{
  for (const dropped of ["--disable-network", "--disable-autodetect"]) {
    const config = CONFIG_ARGS.filter((token) => token !== dropped)
      .map(embedConfigureArg)
      .join(" ");
    const parsed = parseProbeOutput(legalOutput({ config }));
    const verdict = classifyComponents(parsed); // deliberately no expectedConfigure
    assert(
      verdict.violations.some((entry) => entry.includes(`lacks ${dropped}`)),
      `classifyComponents rejects a config missing ${dropped} even without expectedConfigure`,
      JSON.stringify(verdict.violations),
    );
  }
}

// --- mpv API version must be a real positive integer (item 4) --------------
{
  for (const mpv of ["", "0", "NaN", "abc"]) {
    const error = throws(() => parseProbeOutput(legalOutput({ mpv })));
    assert(
      error !== null && error.message.includes("libmpv API version"),
      `an invalid mpv line (${JSON.stringify(mpv)}) fails instead of becoming 0/NaN`,
      error?.message,
    );
  }
  assert(
    parseProbeOutput(legalOutput({ mpv: "131072" })).mpvApiVersion === 131072,
    "a valid mpv API version parses as a positive integer",
  );
  // A missing mpv line entirely must also fail.
  const noMpv = legalOutput().split("\n").filter((line) => !line.startsWith("mpv\t")).join("\n");
  assert(
    throws(() => parseProbeOutput(noMpv)) !== null,
    "a probe output with no mpv line fails",
  );
}

// --- expectedMpvApiVersion assertion (item 5) ------------------------------
{
  const parsed = parseProbeOutput(legalOutput());
  assert(
    classifyComponents(parsed, { expectedMpvApiVersion: 131072 }).violations.length === 0,
    "classifyComponents accepts a matching expectedMpvApiVersion",
  );
  const wrong = classifyComponents(parsed, { expectedMpvApiVersion: 65536 });
  assert(
    wrong.violations.some(
      (entry) => entry.includes("131072") && entry.includes("65536") && entry.includes("difference 65536"),
    ),
    "a mismatched expectedMpvApiVersion is rejected naming both values and the difference",
    JSON.stringify(wrong.violations),
  );
  const error = throws(() =>
    assertMinimalComponents(parsed, { expectedMpvApiVersion: 999999 }),
  );
  assert(error !== null, "assertMinimalComponents forwards expectedMpvApiVersion", error?.message);
}

// --- counts and empty probe runs -------------------------------------------
{
  const parsed = parseProbeOutput(legalOutput());
  parsed.decoders = [];
  const verdict = classifyComponents(parsed);
  assert(
    verdict.violations.some((entry) => entry.includes("required decoder missing")),
    "classifyComponents reports an empty set as violations, not a crash",
  );
}

{
  assert(throws(() => parseProbeOutput("")) !== null, "empty probe output fails instead of passing");
  assert(
    throws(() => parseProbeOutput("version\t6.0\nconfig\tx\nmpv\t131072\n")) !== null,
    "zero components fail",
  );
  assert(throws(() => parseProbeOutput("nonsense line")) !== null, "unknown probe lines are rejected");
  assert(throws(() => parseProbeOutput(legalOutput({ omitCounts: true }))) !== null, "missing counts fail");
  assert(
    throws(() =>
      parseProbeOutput(
        legalOutput({
          countsOverride: {
            filters: 10,
            decoders: 999,
            encoders: 0,
            demuxers: 6,
            muxers: 0,
            protocols_in: 1,
            protocols_out: 1,
          },
        }),
      ),
    ) !== null,
    "a counts line that disagrees with the parsed lines fails",
  );
}

// --- forbidden linkage ------------------------------------------------------
{
  const clean = assertForbiddenLinkage(
    "libmpv.dylib:\n\t@loader_path/libavcodec.dylib\n" +
      "\t/System/Library/Frameworks/CoreAudio.framework/Versions/A/CoreAudio\n" +
      "\t/System/Library/Frameworks/CoreFoundation.framework/Versions/A/CoreFoundation\n",
  );
  assert(clean.length === 0, "CoreAudio/CoreFoundation do not count as forbidden video backends");

  for (const [label, reference] of [
    ["OpenGL", "/System/Library/Frameworks/OpenGL.framework/Versions/A/OpenGL"],
    ["AppKit", "/System/Library/Frameworks/AppKit.framework/Versions/C/AppKit"],
    ["VideoToolbox", "/System/Library/Frameworks/VideoToolbox.framework/Versions/A/VideoToolbox"],
    ["CoreVideo", "/System/Library/Frameworks/CoreVideo.framework/Versions/A/CoreVideo"],
    ["Metal", "/System/Library/Frameworks/Metal.framework/Versions/A/Metal"],
    ["QuartzCore", "/System/Library/Frameworks/QuartzCore.framework/Versions/A/QuartzCore"],
    ["OpenSSL", "/opt/homebrew/opt/openssl@3/lib/libssl.3.dylib"],
    ["GnuTLS", "/opt/homebrew/opt/gnutls/lib/libgnutls.30.dylib"],
    ["mbedTLS", "@loader_path/libmbedtls.14.dylib"],
  ]) {
    const error = throws(() => assertForbiddenLinkage(`libmpv.dylib:\n\t${reference}\n`));
    assert(
      error !== null && error.message.includes(label),
      `forbidden linkage ${label} is rejected and named`,
      error?.message,
    );
  }
}

// --- otool -L header lines must not be judged as dependencies (item 1) -----
{
  // These two real header lines used to be false positives: the OpenSSL install
  // prefix was flagged as OpenSSL, and a build living under .../metal-build/...
  // was flagged as Metal. Both are just the image's own path, not a dependency.
  for (const header of [
    "/opt/homebrew/opt/openssl@3/lib/libcrypto.3.dylib:",
    "/Users/xian/metal-build/libmpv.dylib:",
  ]) {
    const output = `${header}\n\t/System/Library/Frameworks/CoreAudio.framework/Versions/A/CoreAudio\n`;
    assert(
      throws(() => assertForbiddenLinkage(output)) === null,
      `an otool -L header line is ignored: ${header}`,
    );
  }
  // The three real Metal dependencies that /\/metal\b/ missed entirely. They
  // must now be rejected and named "Metal".
  for (const reference of [
    "@rpath/libAGXMetal.dylib",
    "/System/Library/Frameworks/Metal.framework/Versions/A/Metal",
    "/System/Library/Frameworks/MetalPerformanceShaders.framework/Versions/A/MetalPerformanceShaders",
  ]) {
    const error = throws(() => assertForbiddenLinkage(`libmpv.dylib:\n\t${reference}\n`));
    assert(
      error !== null && error.message.includes("Metal"),
      `real Metal dependency is rejected and named: ${reference}`,
      error?.message,
    );
  }
}

// --- empty otool output must fail closed (item 2) --------------------------
{
  for (const [label, output] of [
    ["empty string", ""],
    ["whitespace only", "   \n\t\n"],
    ["header only", "libmpv.dylib:\n"],
  ]) {
    const error = throws(() => assertForbiddenLinkage(output));
    assert(error !== null, `assertForbiddenLinkage fails on ${label}`, error?.message);
  }
}

// --- assertProbeLoadedFrom (item 9) ----------------------------------------
{
  const scratch = mkdtempSync(resolve(tmpdir(), "echo-probe-selftest-"));
  try {
    const hostPath = "/opt/homebrew/Cellar/ffmpeg/9.0.2_1/lib/libavcodec.dylib";
    const symbols = [
      "avcodec_configuration",
      "avfilter_filter_pad_count",
      "av_demuxer_iterate",
      "mpv_client_api_version",
    ];
    const hostLoaded = {
      ...parseProbeOutput(legalOutput()),
      loaded: symbols.map((symbol, index) => ({
        symbol,
        path: index === 0 ? hostPath : resolve(scratch, `lib${index}.dylib`),
      })),
    };
    const hostError = throws(() => assertProbeLoadedFrom(hostLoaded, scratch));
    assert(
      hostError !== null && hostError.message.includes("avcodec_configuration"),
      "a dladdr image outside the vendor directory fails and names the offending symbol",
      hostError?.message,
    );

    // Create real files inside the scratch dir and point every loaded image at
    // them: realpathSync must succeed and all paths must fall under the root.
    const legalLoaded = {
      ...parseProbeOutput(legalOutput()),
      loaded: symbols.map((symbol, index) => {
        const path = resolve(scratch, `lib${index}.dylib`);
        writeFileSync(path, "fixture");
        return { symbol, path };
      }),
    };
    assert(
      throws(() => assertProbeLoadedFrom(legalLoaded, scratch)) === null,
      "loaded images all inside the vendor directory pass",
    );
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

process.stdout.write(
  failures === 0
    ? "\nffmpeg-component-probe self-test: all ok\n"
    : `\nffmpeg-component-probe self-test: ${failures} failure(s)\n`,
);
process.exit(failures === 0 ? 0 : 1);

function legalOutputDecoders() {
  return [
    ["mp3", 1],
    ["mp3float", 1],
    ["flac", 1],
    ["ape", 1],
    ["aac", 1],
    ["aac_latm", 1],
    ["alac", 1],
    ["vorbis", 1],
    ["opus", 1],
    ["pcm_s16le", 1],
    ["pcm_s24le", 1],
    ["pcm_f32le", 1],
  ];
}
