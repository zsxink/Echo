#!/usr/bin/env node
// Compile, run and judge the runtime component probe for the macOS libmpv
// stack.
//
// The Gate must prove what the shipped dylibs register, not what strings they
// contain: after strip, nm has no local symbols, and short words like "scale"
// appear in unrelated error messages. The probe enumerates the live
// registries, and this module turns its TSV into pass/fail verdicts. All
// judging functions are pure so the injection self-test runs on every platform
// (9.7); only runProbe/CLI need macOS.

import { mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, resolve, sep } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
const PROBE_SOURCE = resolve(ROOT, "scripts", "release", "ffmpeg-component-probe.c");

export const MEDIA_TYPE_VIDEO = 0;
export const MEDIA_TYPE_AUDIO = 1;

export const REQUIRED_DECODERS = [
  "mp3",
  "mp3float",
  "flac",
  "ape",
  "aac",
  "aac_latm",
  "alac",
  "vorbis",
  "opus",
  "pcm_s16le",
  "pcm_s24le",
  "pcm_f32le",
];

export const REQUIRED_DEMUXERS = ["mp3", "flac", "ape", "mov", "ogg", "wav"];

export const ALLOWED_DEMUXER_TOKENS = [
  "mp3",
  "flac",
  "ape",
  "mov",
  "mp4",
  "m4a",
  "3gp",
  "3g2",
  "mj2",
  "ogg",
  "wav",
];

export const REQUIRED_FILTERS = [
  "aformat",
  "alimiter",
  "equalizer",
  "extrastereo",
  "volume",
  // mpv auto-inserts aresample when the effect graph's input format differs
  // from `aformat`; without it the whole chain fails to configure.
  "aresample",
];

// FFmpeg 6.0's print_enabled_components appends these four graph endpoints
// unconditionally, and libavfilter/Makefile always compiles buffersrc.o /
// buffersink.o. They must be allowed, but buffer/buffersink are video *graph
// endpoints*, not video processing filters: rejecting them as video would make
// every stock minimal build fail.
export const ENDPOINT_FILTERS = ["abuffer", "abuffersink", "buffer", "buffersink"];

export const ALLOWED_FILTERS = [...REQUIRED_FILTERS, ...ENDPOINT_FILTERS];

export const ALLOWED_PROTOCOLS = ["file"];

// The minimal build enables no encoders or muxers. Keeping these lists
// explicit (rather than only asserting "no video") makes the intent auditable.
export const ALLOWED_ENCODERS = [];
export const ALLOWED_MUXERS = [];

export const FORBIDDEN_LINKAGE = [
  { label: "OpenGL", pattern: /opengl/i },
  { label: "AppKit", pattern: /appkit/i },
  { label: "VideoToolbox", pattern: /videotoolbox/i },
  { label: "CoreVideo", pattern: /corevideo/i },
  // Match Metal in a library/framework *name*, not in an arbitrary path
  // segment: a build living under /Users/.../metal-build must not be flagged,
  // while libAGXMetal.dylib / Metal.framework / MetalPerformanceShaders.framework
  // must be. (\b would miss the libAGXMetal / MetalPerformanceShaders cases.)
  { label: "Metal", pattern: /metal[a-z0-9_.-]*\.(?:framework|dylib)/i },
  { label: "QuartzCore", pattern: /quartzcore/i },
  { label: "OpenSSL", pattern: /libssl|libcrypto|openssl/i },
  { label: "GnuTLS", pattern: /gnutls/i },
  { label: "mbedTLS", pattern: /mbedtls|mbedcrypto/i },
  { label: "libtls", pattern: /libtls/i },
];

// A forbidden configure option is a whole token: it ends at `=`, whitespace or
// end of string. A plain `\b` did NOT end at the next letter/digit, so
// `--enable-gpl3` / `--enable-gplfoo` slipped past the old GPL rule. The
// trailing `[0-9a-z_-]*` absorbs those suffix variants (and any future ones)
// instead of letting them bypass. `--enable-version3` is the switch that
// upgrades FFmpeg to GPLv3; the change is LGPL-only, so it is forbidden too.
const FORBIDDEN_CONFIGURE_OPTIONS = [
  /--enable-gpl[0-9a-z_-]*(?==|\s|$)/i,
  /--enable-version3(?==|\s|$)/i,
  /--enable-nonfree[0-9a-z_-]*(?==|\s|$)/i,
  /--enable-videotoolbox[0-9a-z_-]*(?==|\s|$)/i,
  /--enable-audiotoolbox[0-9a-z_-]*(?==|\s|$)/i,
  /--enable-(?:openssl|gnutls|mbedtls|securetransport)[0-9a-z_-]*(?==|\s|$)/i,
];

// Each allowed filter's pad contract. "none" means the pad array must be
// empty; an audio/video value means every pad must have that type and there
// must be at least one. Empty arrays are never accepted by accident: the old
// probe walked non-terminated FFmpeg pad arrays, so a broken enumeration could
// look like a valid empty pad list.
const FILTER_PAD_CONTRACT = {
  aformat: { inputs: MEDIA_TYPE_AUDIO, outputs: MEDIA_TYPE_AUDIO },
  alimiter: { inputs: MEDIA_TYPE_AUDIO, outputs: MEDIA_TYPE_AUDIO },
  aresample: { inputs: MEDIA_TYPE_AUDIO, outputs: MEDIA_TYPE_AUDIO },
  equalizer: { inputs: MEDIA_TYPE_AUDIO, outputs: MEDIA_TYPE_AUDIO },
  extrastereo: { inputs: MEDIA_TYPE_AUDIO, outputs: MEDIA_TYPE_AUDIO },
  volume: { inputs: MEDIA_TYPE_AUDIO, outputs: MEDIA_TYPE_AUDIO },
  abuffer: { inputs: "none", outputs: MEDIA_TYPE_AUDIO },
  abuffersink: { inputs: MEDIA_TYPE_AUDIO, outputs: "none" },
  buffer: { inputs: "none", outputs: MEDIA_TYPE_VIDEO },
  buffersink: { inputs: MEDIA_TYPE_VIDEO, outputs: "none" },
};

function fail(message) {
  throw new Error(message);
}

export function parsePadTypes(field, label = "filter pad") {
  if (field === "-") return [];
  if (field === undefined || field === null || field === "") {
    fail(`${label} has no pad type field`);
  }
  return String(field).split(",").map((value) => {
    if (!/^-?\d+$/.test(value)) fail(`unparseable ${label} pad type: ${field}`);
    return Number(value);
  });
}

function padsMatch(types, expected) {
  if (expected === "none") return types.length === 0;
  return types.length > 0 && types.every((type) => type === expected);
}

function padLabel(types) {
  return `[${types.join(",")}]`;
}

function parseCounts(field) {
  if (!field) fail("probe counts line is empty");
  const counts = {};
  for (const token of field.split(/\s+/)) {
    const match = /^([a-z_]+)=(\d+)$/.exec(token);
    if (!match) fail(`unparseable probe count: ${token}`);
    if (Object.hasOwn(counts, match[1])) fail(`duplicate probe count: ${match[1]}`);
    counts[match[1]] = Number(match[2]);
  }
  return counts;
}

export function parseProbeOutput(output) {
  const parsed = {
    version: null,
    config: null,
    mpvApiVersion: null,
    loaded: [],
    decoders: [],
    encoders: [],
    filters: [],
    demuxers: [],
    muxers: [],
    protocolsIn: [],
    protocolsOut: [],
    counts: null,
  };
  for (const line of String(output ?? "").split("\n")) {
    if (!line) continue;
    const fields = line.split("\t");
    switch (fields[0]) {
      case "version":
        parsed.version = fields[1] ?? null;
        break;
      case "config":
        parsed.config = fields.slice(1).join("\t");
        break;
      case "mpv": {
        // Fail closed on the value itself: Number("") === 0 and Number("abc")
        // is NaN, and a NaN/empty "mpv" line used to sail through the old
        // Number.isInteger check as a no-op. mpv_client_api_version() returns
        // an unsigned long whose high word is the major version, so any real
        // answer is >= 0x10000 (major >= 1).
        const raw = fields[1];
        const value = raw === undefined || raw === "" ? NaN : Number(raw);
        if (!Number.isInteger(value) || value < 0x10000) {
          fail(`probe produced an invalid libmpv API version: ${JSON.stringify(raw ?? null)}`);
        }
        parsed.mpvApiVersion = value;
        break;
      }
      case "loaded":
        if (!fields[1] || !fields[2]) fail("loaded line without a symbol or path");
        parsed.loaded.push({ symbol: fields[1], path: fields[2] });
        break;
      case "decoder":
        if (!fields[1]) fail("decoder line without a name");
        parsed.decoders.push({ name: fields[1], type: Number(fields[2]) });
        break;
      case "encoder":
        if (!fields[1]) fail("encoder line without a name");
        parsed.encoders.push({ name: fields[1], type: Number(fields[2]) });
        break;
      case "filter":
        if (!fields[1]) fail("filter line without a name");
        parsed.filters.push({
          name: fields[1],
          inputs: parsePadTypes(fields[2], `${fields[1]} input`),
          outputs: parsePadTypes(fields[3], `${fields[1]} output`),
        });
        break;
      case "demuxer":
        if (!fields[1]) fail("demuxer line without a name");
        parsed.demuxers.push({ name: fields[1] });
        break;
      case "muxer":
        if (!fields[1]) fail("muxer line without a name");
        parsed.muxers.push({ name: fields[1] });
        break;
      case "protocol_in":
        if (!fields[1]) fail("protocol_in line without a name");
        parsed.protocolsIn.push(fields[1]);
        break;
      case "protocol_out":
        if (!fields[1]) fail("protocol_out line without a name");
        parsed.protocolsOut.push(fields[1]);
        break;
      case "counts":
        if (parsed.counts) fail("duplicate probe counts line");
        parsed.counts = parseCounts(fields[1]);
        break;
      default:
        fail(`unknown probe line: ${line.slice(0, 80)}`);
    }
  }
  if (!parsed.version) fail("probe produced no FFmpeg version");
  if (!parsed.config) fail("probe produced no embedded configure string");
  if (!Number.isInteger(parsed.mpvApiVersion) || parsed.mpvApiVersion < 0x10000) {
    fail("probe produced no valid libmpv API version");
  }
  if (parsed.loaded.length === 0) fail("probe reported no loaded images");
  for (const section of ["decoders", "filters", "demuxers", "protocolsIn"]) {
    if (parsed[section].length === 0) {
      fail(`probe produced zero ${section}; an empty enumeration must not pass`);
    }
  }
  if (!parsed.counts) fail("probe produced no counts line");

  const actualCounts = {
    filters: parsed.filters.length,
    decoders: parsed.decoders.length,
    encoders: parsed.encoders.length,
    demuxers: parsed.demuxers.length,
    muxers: parsed.muxers.length,
    protocols_in: parsed.protocolsIn.length,
    protocols_out: parsed.protocolsOut.length,
  };
  const countKeys = Object.keys(actualCounts).sort();
  const reportedKeys = Object.keys(parsed.counts).sort();
  if (JSON.stringify(countKeys) !== JSON.stringify(reportedKeys)) {
    fail(`probe counts keys are [${reportedKeys.join(", ")}], expected [${countKeys.join(", ")}]`);
  }
  for (const key of countKeys) {
    if (parsed.counts[key] !== actualCounts[key]) {
      fail(`probe counts ${key}=${parsed.counts[key]} disagrees with ${actualCounts[key]} parsed line(s)`);
    }
  }
  return parsed;
}

// avcodec_configuration() embeds the original configure argv, but values with
// commas are quoted with shell single quotes. Tokenising here keeps the
// comparison against FFMPEG_CONFIGURE_ARGS exact without depending on the
// caller's shell.
export function parseConfigureString(text) {
  const tokens = [];
  let current = "";
  let quote = null;
  let escaped = false;
  for (const char of String(text ?? "")) {
    if (escaped) {
      current += char;
      escaped = false;
    } else if (char === "\\" && quote !== "'") {
      escaped = true;
    } else if (quote) {
      if (char === quote) quote = null;
      else current += char;
    } else if (char === "'" || char === '"') {
      quote = char;
    } else if (/\s/.test(char)) {
      if (current) {
        tokens.push(current);
        current = "";
      }
    } else {
      current += char;
    }
  }
  if (escaped) current += "\\";
  if (quote) fail(`unterminated quote in configure string: ${text}`);
  if (current) tokens.push(current);
  return tokens;
}

export function assertConfigureMatches(config, expectedArgs) {
  const actual = parseConfigureString(config);
  const expected = [...expectedArgs];
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    const firstDifference = actual.findIndex((value, index) => value !== expected[index]);
    const index = firstDifference === -1 ? Math.min(actual.length, expected.length) : firstDifference;
    fail(
      `embedded configure arguments drift from the build script at token ${index}: ` +
        `actual ${JSON.stringify(actual[index] ?? "<missing>")}, expected ${JSON.stringify(expected[index] ?? "<missing>")}`,
    );
  }
  return actual;
}

export function classifyComponents(
  parsed,
  { expectedVersion = "6.0", expectedConfigure, expectedMpvApiVersion } = {},
) {
  const violations = [];

  if (parsed.version !== expectedVersion) {
    violations.push(`FFmpeg version ${parsed.version}, expected ${expectedVersion}`);
  }
  if (expectedMpvApiVersion !== undefined) {
    // The mpv API version is a probe-of-record value: assert it explicitly
    // (with the concrete numbers) instead of leaving it unvalidated.
    if (parsed.mpvApiVersion !== expectedMpvApiVersion) {
      violations.push(
        `libmpv API version ${parsed.mpvApiVersion}, expected ${expectedMpvApiVersion} ` +
          `(difference ${parsed.mpvApiVersion - expectedMpvApiVersion})`,
      );
    }
  }
  // These three are the supply-chain invariants of an audio-only build. Assert
  // them on the embedded configure string's tokens, even when no
  // expectedConfigure was supplied: the per-token comparison below only covers
  // them indirectly. Tokenising (rather than a substring match) also stops
  // `--disable-network=foo` or a coincidental path fragment from counting.
  const configTokens = parseConfigureString(parsed.config);
  for (const flag of ["--disable-everything", "--disable-network", "--disable-autodetect"]) {
    if (!configTokens.includes(flag)) {
      violations.push(`embedded configure string lacks ${flag}`);
    }
  }
  for (const pattern of FORBIDDEN_CONFIGURE_OPTIONS) {
    const match = pattern.exec(parsed.config);
    if (match) {
      violations.push(`embedded configure string contains forbidden option ${match[0]}`);
    }
  }
  if (expectedConfigure) {
    try {
      assertConfigureMatches(parsed.config, expectedConfigure);
    } catch (error) {
      violations.push(error.message);
    }
  }

  const decoderNames = new Set(parsed.decoders.map((entry) => entry.name));
  for (const decoder of parsed.decoders) {
    if (decoder.type !== MEDIA_TYPE_AUDIO) {
      violations.push(`non-audio decoder registered: ${decoder.name} (type ${decoder.type})`);
    }
    if (!REQUIRED_DECODERS.includes(decoder.name) && !decoder.name.startsWith("pcm_")) {
      violations.push(`unexpected decoder registered: ${decoder.name}`);
    }
  }
  for (const required of REQUIRED_DECODERS) {
    if (!decoderNames.has(required)) violations.push(`required decoder missing: ${required}`);
  }
  if (![...decoderNames].some((name) => name.startsWith("pcm_"))) {
    violations.push("PCM decoder family is missing");
  }

  for (const encoder of parsed.encoders) {
    if (!ALLOWED_ENCODERS.includes(encoder.name)) {
      violations.push(`unexpected encoder registered: ${encoder.name} (type ${encoder.type})`);
    }
  }

  const filterNames = new Set(parsed.filters.map((entry) => entry.name));
  for (const filter of parsed.filters) {
    if (!ALLOWED_FILTERS.includes(filter.name)) {
      violations.push(`unexpected filter registered: ${filter.name}`);
      continue;
    }
    const contract = FILTER_PAD_CONTRACT[filter.name];
    if (!contract) {
      violations.push(`allowed filter ${filter.name} has no declared pad contract`);
      continue;
    }
    if (!padsMatch(filter.inputs, contract.inputs)) {
      violations.push(
        `filter ${filter.name} has unexpected input pads ${padLabel(filter.inputs)} ` +
          `(expected ${contract.inputs === "none" ? "none" : contract.inputs})`,
      );
    }
    if (!padsMatch(filter.outputs, contract.outputs)) {
      violations.push(
        `filter ${filter.name} has unexpected output pads ${padLabel(filter.outputs)} ` +
          `(expected ${contract.outputs === "none" ? "none" : contract.outputs})`,
      );
    }
  }
  for (const required of REQUIRED_FILTERS) {
    if (!filterNames.has(required)) violations.push(`required filter missing: ${required}`);
  }
  for (const endpoint of ENDPOINT_FILTERS) {
    if (!filterNames.has(endpoint)) violations.push(`required graph endpoint missing: ${endpoint}`);
  }

  const demuxerTokens = new Set(parsed.demuxers.flatMap((entry) => entry.name.split(",")));
  for (const token of demuxerTokens) {
    if (!ALLOWED_DEMUXER_TOKENS.includes(token)) {
      violations.push(`unexpected demuxer registered: ${token}`);
    }
  }
  for (const required of REQUIRED_DEMUXERS) {
    if (!demuxerTokens.has(required)) violations.push(`required demuxer missing: ${required}`);
  }

  const muxerTokens = new Set(parsed.muxers.flatMap((entry) => entry.name.split(",")));
  for (const token of muxerTokens) {
    if (!ALLOWED_MUXERS.includes(token)) {
      violations.push(`unexpected muxer registered: ${token}`);
    }
  }

  for (const [label, actual] of [
    ["input", parsed.protocolsIn],
    ["output", parsed.protocolsOut],
  ]) {
    const protocols = [...actual].sort();
    if (
      protocols.length !== ALLOWED_PROTOCOLS.length ||
      protocols.some((name, index) => name !== ALLOWED_PROTOCOLS[index])
    ) {
      violations.push(
        `${label} protocols are [${protocols.join(", ")}], expected [${ALLOWED_PROTOCOLS.join(", ")}]`,
      );
    }
  }

  return {
    counts: {
      decoders: parsed.decoders.length,
      encoders: parsed.encoders.length,
      filters: parsed.filters.length,
      demuxers: parsed.demuxers.length,
      muxers: parsed.muxers.length,
      protocolsIn: parsed.protocolsIn.length,
      protocolsOut: parsed.protocolsOut.length,
    },
    violations: violations.sort(),
  };
}

export function assertMinimalComponents(parsed, options) {
  const verdict = classifyComponents(parsed, options);
  if (verdict.violations.length > 0) {
    // Put the components a reviewer expects to see first; a full build can
    // produce a thousand violations, and hiding h264/scale/matroska behind the
    // first 40 alphabetical entries would make the failure unreadable.
    const notable = verdict.violations.filter((violation) =>
      ["h264", "hevc", "prores", "scale", "overlay", "matroska", "mpegts"].some((name) =>
        new RegExp(`\\b${name}\\b`).test(violation),
      ),
    );
    const ordered = [...notable, ...verdict.violations.filter((violation) => !notable.includes(violation))];
    const shown = ordered.slice(0, 40);
    const hidden = ordered.length - shown.length;
    fail(
      `component probe rejected the build:\n- ${shown.join("\n- ")}` +
        (hidden > 0 ? `\n- ...and ${hidden} more violation(s)` : ""),
    );
  }
  return verdict;
}

export function assertForbiddenLinkage(otoolOutput) {
  const text = String(otoolOutput ?? "");
  // `otool -L` prints a non-indented header line ending in `:` (the image's own
  // path) followed by indented dependency lines. Judging the whole output
  // mistook header paths for dependencies: a build under .../metal-build/... was
  // flagged as Metal, and an OpenSSL install prefix in the header was flagged as
  // OpenSSL. Dependency lines are indented; the header is the only unindented
  // line, so keep only the indented lines.
  const dependencyLines = text.split("\n").filter((line) => /^\s+\S/.test(line));
  if (dependencyLines.length === 0) {
    // Fail closed: an empty/blank otool output means the scan never happened.
    // task-1.10's run() checks spawnSync status, but this pure function must
    // also refuse to pass when it is handed nothing to inspect.
    fail("otool -L produced no dependency lines to inspect");
  }
  const textToScan = dependencyLines.join("\n");
  const matches = FORBIDDEN_LINKAGE.filter((entry) => entry.pattern.test(textToScan)).map(
    (entry) => entry.label,
  );
  if (matches.length > 0) {
    fail(`forbidden linkage found: ${[...new Set(matches)].join(", ")}`);
  }
  return matches;
}

export function assertProbeLoadedFrom(parsed, vendorDir) {
  const root = realpathSync(resolve(vendorDir));
  const requiredSymbols = [
    "avcodec_configuration",
    "avfilter_filter_pad_count",
    "av_demuxer_iterate",
    "mpv_client_api_version",
  ];
  for (const symbol of requiredSymbols) {
    if (!parsed.loaded.some((entry) => entry.symbol === symbol)) {
      fail(`probe did not report the loaded image for ${symbol}`);
    }
  }
  for (const entry of parsed.loaded) {
    let actual;
    try {
      actual = realpathSync(resolve(entry.path));
    } catch {
      fail(`probe loaded image is unreadable: ${entry.symbol} -> ${entry.path}`);
    }
    if (actual !== root && !actual.startsWith(`${root}${sep}`)) {
      fail(
        `probe loaded ${entry.symbol} from ${actual}, outside the vendor directory ${root}; ` +
          "the Gate must inspect the shipped dylibs, not a host copy",
      );
    }
  }
}

function run(command, args, options = {}) {
  return spawnSync(command, args, {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    ...options,
  });
}

export function runProbe(vendorDir) {
  const dir = resolve(vendorDir);
  const scratch = mkdtempSync(resolve(tmpdir(), "echo-ffmpeg-probe-"));
  const binary = resolve(scratch, "probe");
  try {
    const compile = run("cc", [
      "-O2",
      "-o",
      binary,
      PROBE_SOURCE,
      `-L${dir}`,
      "-lavformat",
      "-lavcodec",
      "-lavfilter",
      "-lavutil",
      "-lmpv",
      `-Wl,-rpath,${dir}`,
    ]);
    if (compile.status !== 0) {
      fail(`probe compilation failed:\n${compile.stdout || ""}${compile.stderr || ""}`);
    }
    const execution = run(binary, [], { env: { ...process.env, DYLD_LIBRARY_PATH: dir } });
    if (execution.status !== 0) {
      fail(`probe execution failed:\n${execution.stdout || ""}${execution.stderr || ""}`);
    }
    const parsed = parseProbeOutput(execution.stdout);
    assertProbeLoadedFrom(parsed, dir);
    return parsed;
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

function otoolLibraries(file) {
  const result = run("otool", ["-L", file]);
  if (result.status !== 0) {
    fail(`otool -L failed for ${file}:\n${result.stdout || ""}${result.stderr || ""}`);
  }
  return result.stdout || "";
}

// The final Gate before publishing (build-macos-libmpv.mjs writeVendor) calls
// this CLI with only `--vendor <dir>`. Read the expected FFmpeg version and
// configure tokens from that same vendor's manifest.json; importing the build
// script here would invert its dependency on us and create a cycle.
function expectedOptionsFromManifest(vendor) {
  const manifestPath = resolve(vendor, "manifest.json");
  let manifest;
  try {
    manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  } catch (error) {
    fail(`cannot read ${manifestPath}: ${error.message}`);
  }
  const version = manifest?.builds?.ffmpeg?.version;
  const configure = manifest?.builds?.ffmpeg?.configure;
  if (typeof version !== "string" || version === "") {
    fail(`${manifestPath} has no builds.ffmpeg.version`);
  }
  if (!Array.isArray(configure)) {
    fail(`${manifestPath} builds.ffmpeg.configure is not an array`);
  }
  return { expectedVersion: version, expectedConfigure: configure };
}

function main(argv) {
  const index = argv.indexOf("--vendor");
  if (index === -1 || !argv[index + 1]) {
    fail("usage: ffmpeg-component-probe.mjs --vendor <dir>");
  }
  const vendor = resolve(argv[index + 1]);
  const expected = expectedOptionsFromManifest(vendor);
  const parsed = runProbe(vendor);
  const verdict = assertMinimalComponents(parsed, expected);
  const dylibs = readdirSync(vendor)
    .filter((entry) => entry.endsWith(".dylib"))
    .sort();
  if (dylibs.length === 0) fail(`no dylibs found in ${vendor}`);
  let forbiddenLinks = 0;
  for (const filename of dylibs) {
    forbiddenLinks += assertForbiddenLinkage(otoolLibraries(resolve(vendor, filename))).length;
  }
  if (!dylibs.includes("libmpv.dylib")) fail(`libmpv.dylib is missing from ${vendor}`);
  process.stdout.write(
    `ok component probe: FFmpeg ${parsed.version}, ` +
      `${verdict.counts.decoders} decoders / ${verdict.counts.filters} filters / ` +
      `${verdict.counts.demuxers} demuxers / ${verdict.counts.muxers} muxers / ` +
      `${verdict.counts.encoders} encoders / ` +
      `protocols in [${parsed.protocolsIn.join(", ")}] out [${parsed.protocolsOut.join(", ")}]; ` +
      `${forbiddenLinks} forbidden links across ${dylibs.length} dylibs (${basename(vendor)})\n`,
  );
}

const invoked = process.argv[1] ? resolve(process.argv[1]) : "";
if (invoked === fileURLToPath(import.meta.url)) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    process.stderr.write(`FAIL ffmpeg-component-probe: ${error.message}\n`);
    process.exit(1);
  }
}
