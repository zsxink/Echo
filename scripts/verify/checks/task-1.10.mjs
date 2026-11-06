#!/usr/bin/env node
// macOS libmpv provenance and bundle report Gate.

import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import {
  classifyArchitectures,
  computeVendorClosure,
  parseDylibReferences,
  parseInstallNames,
  parseLipoArchs,
} from "../../release/thin-macos-libmpv.mjs";
import {
  deploymentTargetViolation,
  FFMPEG_CONFIGURE_ARGS,
  MACOSX_DEPLOYMENT_TARGET,
  MPV_HARD_DEPENDENCIES,
  MPV_MESON_OPTIONS,
  parseMinos,
  renderNoticeBlock,
  summarizeFfmpegBuild,
  summarizeMpvBuild,
} from "../../release/build-macos-libmpv.mjs";
import {
  assertForbiddenLinkage,
  assertMinimalComponents,
  classifyComponents,
  runProbe,
} from "../../release/ffmpeg-component-probe.mjs";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const VENDOR = resolve(ROOT, "apps", "desktop", "src-tauri", "vendor", "libmpv", "macos");
const DEFAULT_BUNDLE = resolve(
  ROOT,
  "target",
  "aarch64-apple-darwin",
  "release",
  "bundle",
  "macos",
  "Echo.app",
  "Contents",
);
const BUNDLE = resolve(process.env.ECHO_MACOS_BUNDLE || DEFAULT_BUNDLE);
const REPORT = resolve(
  process.env.ECHO_MACOS_REPORT ||
    resolve(ROOT, "target", "aarch64-apple-darwin", "release", "bundle", "macos", "echo-libmpv-report.md"),
);
const CI_WORKFLOW = resolve(ROOT, ".github", "workflows", "ci.yml");
const RELEASE_WORKFLOW = resolve(ROOT, ".github", "workflows", "release.yml");

const NOTICE_BEGIN = "<!-- BEGIN GENERATED AUXILIARY VERSIONS -->";
const NOTICE_END = "<!-- END GENERATED AUXILIARY VERSIONS -->";

// mpv's MPV_CLIENT_API_VERSION is (major << 16) | minor: a 2-part packed
// value, not major.minor.patch. mpv 0.36.0 declares client API 2.1, i.e.
// 0x20001 = 131073 — verified against mpv_client_api_version() of the
// vendored dylib. A build that reports a different ABI is not the pinned mpv.
const MPV_CLIENT_API_VERSIONS = {
  "0.36.0": (2 << 16) | 1, // 0x20001 = 131073
};

function fail(message) {
  process.stderr.write(`FAIL 1.10: ${message}\n`);
  process.exit(1);
}

function digest(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function run(command, args) {
  const result = spawnSync(command, args, { cwd: ROOT, encoding: "utf8" });
  if (result.status !== 0) fail(`${command} ${args.join(" ")} failed\n${result.stdout || ""}${result.stderr || ""}`);
  return `${result.stdout || ""}${result.stderr || ""}`;
}

function dylibNames(dir) {
  if (!existsSync(dir)) fail(`dylib directory is missing: ${dir}`);
  return readdirSync(dir)
    .filter((entry) => entry.endsWith(".dylib"))
    .sort();
}

function assertSameSet(label, expected, actual) {
  const missing = expected.filter((entry) => !actual.includes(entry));
  const extra = actual.filter((entry) => !expected.includes(entry));
  if (missing.length > 0 || extra.length > 0) {
    fail(`${label} mismatch; missing [${missing.join(", ")}], extra [${extra.join(", ")}]`);
  }
}

function assertRpathInstallName(path, expectedFile, label) {
  const names = parseInstallNames(run("otool", ["-D", path]));
  if (names.length !== 1 || names[0] !== `@rpath/${expectedFile}`) {
    fail(
      `${label} install name is [${names.join(", ")}], expected exactly @rpath/${expectedFile}`,
    );
  }
}

function assertHardDependencies(dir, label) {
  const references = {};
  for (const filename of dylibNames(dir)) {
    references[filename] = parseDylibReferences(run("otool", ["-L", resolve(dir, filename)]));
  }
  const closure = computeVendorClosure("libmpv.dylib", references);
  for (const dependency of MPV_HARD_DEPENDENCIES) {
    if (!closure.includes(dependency)) {
      fail(`${label} libmpv dependency closure does not contain hard dependency ${dependency}`);
    }
  }
  return closure;
}

function assertDeploymentTargets(dir, label) {
  for (const filename of dylibNames(dir)) {
    const minos = parseMinos(run("otool", ["-l", resolve(dir, filename)]));
    const violation = deploymentTargetViolation(filename, minos);
    if (violation) fail(`${label} ${violation}`);
  }
}

// Split `otool -l` output into individual `Load command N` blocks. Codesign
// reorders neither the commands nor the sections, so block-scoped parsing is
// stable across vendor (ad-hoc signed once) and bundle (re-signed by Tauri).
function loadCommandBlocks(output) {
  return String(output ?? "")
    .split(/^\s*Load command \d+\s*$/m)
    .slice(1);
}

function field(block, name) {
  const match = new RegExp(`^\\s*${name}\\s+(\\S+)`, "m").exec(block);
  return match ? match[1] : null;
}

// Parse the file-backed section geometry from `otool -l`. `sectname` blocks
// carry (segname, offset, size). Zero-fill sections (__bss) have offset 0 and
// size > 0 but occupy no file bytes, so they are skipped.
function parseSections(output) {
  const lines = String(output ?? "").split("\n");
  const sections = [];
  for (let index = 0; index < lines.length; index += 1) {
    if (!/^\s*sectname /.test(lines[index])) continue;
    const sectname = field(lines.slice(index, index + 12).join("\n"), "sectname");
    const segname = field(lines.slice(index, index + 12).join("\n"), "segname");
    const offset = field(lines.slice(index, index + 12).join("\n"), "offset");
    const sizeRaw = field(lines.slice(index, index + 12).join("\n"), "size");
    if (!sectname || !segname || offset === null || sizeRaw === null) {
      fail(`otool -l section entry is missing segname/offset/size: ${lines[index].trim()}`);
    }
    const offsetValue = Number(offset);
    const size = String(sizeRaw).startsWith("0x") ? parseInt(sizeRaw, 16) : Number(sizeRaw);
    if (!Number.isInteger(offsetValue) || !Number.isInteger(size) || size < 0) {
      fail(`otool -l section ${segname}/${sectname} has unparseable geometry`);
    }
    sections.push({ segname, sectname, offset: offsetValue, size });
  }
  if (sections.length === 0) fail("otool -l reported no sections; cannot bind bundle to vendor");
  return sections;
}

// A whole-file SHA-256 cannot bind the app bundle to the vendor tree: Tauri
// re-signs every bundled dylib, changing the Mach-O header load commands
// (LC_CODE_SIGNATURE) and appending a CodeDirectory/signature blob after
// __LINKEDIT. Even the __TEXT *segment* hash differs, because those load
// commands live in the header region at the start of __TEXT (fileoff 0).
//
// codesign never touches section *contents*. This hashes every file-backed
// section's bytes (plus its name/segment/offset/size), skipping zero-fill
// sections: the two trees must produce identical digests. A stale Framework
// with the same filename but different code cannot match. This is stronger
// than comparing only LC_BUILD_VERSION / LC_ID_DYLIB / __TEXT sizes, which
// would still pass an old-but-structurally-similar build.
function contentDigest(path) {
  const output = run("otool", ["-l", path]);
  const buffer = readFileSync(path);
  const hash = createHash("sha256");
  let counted = 0;
  for (const section of parseSections(output)) {
    if (section.offset === 0 || section.size === 0) continue; // zero-fill section
    const end = section.offset + section.size;
    if (end > buffer.length) {
      fail(`${path} section ${section.segname}/${section.sectname} extends past EOF (${end} > ${buffer.length})`);
    }
    hash.update(`${section.segname},${section.sectname},${section.offset},${section.size}\n`);
    hash.update(buffer.subarray(section.offset, end));
    counted += 1;
  }
  if (counted === 0) fail(`${path} has no file-backed sections to bind`);
  return hash.digest("hex");
}

// Cheap structural cross-checks alongside the content digest. These fields are
// also re-sign-invariant: LC_ID_DYLIB and LC_BUILD_VERSION are load commands
// that codesign preserves, and __TEXT filesize is a segment geometry value.
function bundleProvenance(path) {
  const output = run("otool", ["-l", path]);
  const blocks = loadCommandBlocks(output);
  let idName = null;
  let idCurrent = null;
  let idCompatibility = null;
  let buildVersion = null;
  let textFileSize = null;
  for (const block of blocks) {
    if (!/cmd LC_ID_DYLIB\b/.test(block)) continue;
    const nameLine = /name\s+(\S+)/.exec(block);
    idName = nameLine ? nameLine[1] : null;
    const current = /current version\s+(\S+)/.exec(block);
    const compatibility = /compatibility version\s+(\S+)/.exec(block);
    idCurrent = current ? current[1] : null;
    idCompatibility = compatibility ? compatibility[1] : null;
  }
  for (const block of blocks) {
    if (!/cmd LC_BUILD_VERSION\b/.test(block)) continue;
    buildVersion = ["platform", "minos", "sdk"].map((key) => `${key}=${field(block, key)}`).join(",");
  }
  const text = /segname __TEXT\s+[\s\S]*?filesize\s+(\S+)/.exec(output);
  textFileSize = text ? text[1] : null;
  if (!idName || buildVersion === null || textFileSize === null) {
    fail(`${path} is missing LC_ID_DYLIB / LC_BUILD_VERSION / __TEXT filesize`);
  }
  return { idName, idCurrent, idCompatibility, buildVersion, textFileSize };
}

function assertBundleMatchesVendor(filename) {
  const vendor = resolve(VENDOR, filename);
  const bundled = resolve(BUNDLE, "Frameworks", filename);
  const vendorContent = contentDigest(vendor);
  const bundleContent = contentDigest(bundled);
  if (vendorContent !== bundleContent) {
    fail(
      `bundle ${filename} executable content differs from vendor ` +
        `(section digest ${bundleContent.slice(0, 16)} != ${vendorContent.slice(0, 16)}); ` +
        "the app bundle may be a stale or unapproved build",
    );
  }
  const a = bundleProvenance(vendor);
  const b = bundleProvenance(bundled);
  if (a.idName !== `@rpath/${filename}`) fail(`vendor ${filename} LC_ID_DYLIB is ${a.idName}`);
  if (b.idName !== `@rpath/${filename}`) fail(`bundle ${filename} LC_ID_DYLIB is ${b.idName}`);
  for (const key of ["idCurrent", "idCompatibility", "buildVersion", "textFileSize"]) {
    if (a[key] !== b[key]) {
      fail(`bundle ${filename} ${key} is ${b[key]}, vendor is ${a[key]}`);
    }
  }
  return { vendorContent, bundleContent };
}

// The release Gate step must be a real macOS-only step that points at the exact
// release artifact and runs the bundle Gate after the bundles are built. A
// substring check alone would pass a commented-out, misplaced or Linux-gated
// step, leaving published bytes unvalidated.
function assertReleaseGateStep(text) {
  const lines = text.split("\n");
  const stepHeader = (line) => /^\s*-\s+name:\s*(.+?)\s*$/.exec(line)?.[1]?.replace(/^["']|["']$/g, "");
  let gateIndex = -1;
  let buildIndex = -1;
  for (let index = 0; index < lines.length; index += 1) {
    const name = stepHeader(lines[index]);
    if (!name) continue;
    if (/^Gate release bundles\b/.test(name) && gateIndex === -1) gateIndex = index;
    if (/^Build release bundles\b/.test(name) && buildIndex === -1) buildIndex = index;
  }
  if (gateIndex === -1) fail("release.yml has no 'Gate release bundles' step");
  if (buildIndex === -1) fail("release.yml has no 'Build release bundles' step");
  if (gateIndex <= buildIndex) {
    fail("release.yml 'Gate release bundles' step must come after 'Build release bundles'");
  }
  let nextStep = lines.length;
  for (let index = gateIndex + 1; index < lines.length; index += 1) {
    if (stepHeader(lines[index])) { nextStep = index; break; }
  }
  const block = lines.slice(gateIndex, nextStep).join("\n");
  const ifLine = /^\s*if:\s*(.+)$/m.exec(block)?.[1]?.trim() ?? "";
  if (!ifLine.includes("runner.os == 'macOS'") && !ifLine.includes('runner.os == "macOS"')) {
    fail("release.yml 'Gate release bundles' step is not gated on runner.os == 'macOS'");
  }
  const envMatch = /ECHO_MACOS_BUNDLE:\s*(.+)/.exec(block);
  if (!envMatch || !/target\/release/.test(envMatch[1])) {
    fail("release.yml 'Gate release bundles' ECHO_MACOS_BUNDLE must point under target/release");
  }
  if (!/run:\s*pnpm verify:task -- 1\.10 9\.7\s*$/m.test(block)) {
    fail("release.yml 'Gate release bundles' step must run `pnpm verify:task -- 1.10 9.7`");
  }
  return { gateIndex, buildIndex };
}

function extractNoticeBlock(notice) {
  const begins = notice.split(NOTICE_BEGIN).length - 1;
  const ends = notice.split(NOTICE_END).length - 1;
  if (begins !== 1 || ends !== 1) {
    fail(`NOTICE.md generated block sentinels must appear exactly once (BEGIN x${begins}, END x${ends})`);
  }
  const start = notice.indexOf(NOTICE_BEGIN);
  const end = notice.indexOf(NOTICE_END) + NOTICE_END.length;
  if (end <= start) fail("NOTICE.md generated END sentinel precedes BEGIN sentinel");
  return notice.slice(start, end);
}

if (process.platform !== "darwin") fail("the current Stage 1 Gate is intentionally macOS-local; run it on macOS");

const manifest = JSON.parse(readFileSync(resolve(VENDOR, "manifest.json"), "utf8"));
const builds = manifest.builds;

// Validate structured provenance BEFORE the runtime probe: the probe's expected
// FFmpeg version comes from builds.ffmpeg.version, so a manifest without
// `builds` must fail with a readable message, not a TypeError reading
// `undefined.ffmpeg`. No silent fallback to a weaker check is allowed.
if (!builds || typeof builds !== "object") {
  fail("manifest is missing structured builds provenance");
}
if (!builds.ffmpeg || typeof builds.ffmpeg.version !== "string" || builds.ffmpeg.version.length === 0) {
  fail("manifest builds.ffmpeg.version is missing or empty");
}
if (!Array.isArray(builds.ffmpeg.configure) || builds.ffmpeg.configure.length === 0) {
  fail("manifest builds.ffmpeg.configure must be a non-empty array");
}
if (!builds.mpv || typeof builds.mpv.version !== "string" || builds.mpv.version.length === 0) {
  fail("manifest builds.mpv.version is missing or empty");
}
if (!Array.isArray(builds.auxiliary)) {
  fail("manifest builds.auxiliary must be an array");
}

// mpv version -> client API version pin. Both the vendor and bundle probes must
// report this exact ABI, so a rebuilt/replaced libmpv cannot pass by keeping a
// clean FFmpeg configure string.
const expectedMpvApiVersion = MPV_CLIENT_API_VERSIONS[builds.mpv.version];
if (!Number.isInteger(expectedMpvApiVersion)) {
  fail(`manifest builds.mpv.version ${builds.mpv.version} has no pinned mpv client API version`);
}
// The version must also agree with the pinned mpv source archive URL, so the
// manifest cannot record one version while downloading another.
const mpvSource = (manifest.additionalSources ?? []).find((entry) => entry.name === "mpv");
if (!mpvSource || !String(mpvSource.url ?? "").includes(`v${builds.mpv.version}`)) {
  fail(`manifest builds.mpv.version ${builds.mpv.version} does not match the pinned mpv source URL`);
}

// `classifyComponents` may not expose expectedMpvApiVersion yet (another agent
// is adding it). Detect support from its destructured parameter list so a
// transitional signature cannot make this Gate crash. Regardless, the direct
// assertion below always enforces the pin; the option is passed only when the
// callee understands it (an unknown key is harmless but ignored).
const hasMpvOption = /\{\s*[^}]*\bexpectedMpvApiVersion\b/.test(
  Function.prototype.toString.call(classifyComponents),
);

function assertProbeMpvVersion(parsed, label) {
  if (!Number.isInteger(parsed.mpvApiVersion) || parsed.mpvApiVersion <= 0) {
    fail(`${label} probe reported no positive integer mpv client API version`);
  }
  if (parsed.mpvApiVersion !== expectedMpvApiVersion) {
    fail(
      `${label} mpv client API version ${parsed.mpvApiVersion} != pinned ` +
        `${expectedMpvApiVersion} for mpv ${builds.mpv.version}`,
    );
  }
}

const probeOptions = {
  expectedVersion: builds.ffmpeg.version,
  expectedConfigure: builds.ffmpeg.configure,
  ...(hasMpvOption ? { expectedMpvApiVersion } : {}),
};

// Run the runtime probe: it reads the actual registries, so a vendor that still
// carries video components is rejected by name even when its manifest predates
// the structured build provenance checked above.
const vendorProbe = runProbe(VENDOR);
assertProbeMpvVersion(vendorProbe, "vendor");
assertMinimalComponents(vendorProbe, probeOptions);

const vendorFiles = dylibNames(VENDOR);
const manifestFiles = Object.keys(manifest.files ?? {}).sort();
const fileArchitectureFiles = Object.keys(manifest.fileArchitectures ?? {}).sort();
assertSameSet("manifest.files vs vendor directory", manifestFiles, vendorFiles);
assertSameSet("manifest.fileArchitectures vs manifest.files", manifestFiles, fileArchitectureFiles);

let scannedDylibs = 0;
for (const filename of vendorFiles) {
  assertForbiddenLinkage(run("otool", ["-L", resolve(VENDOR, filename)]));
  scannedDylibs += 1;
}

if (!/^https:\/\//.test(manifest.sourceUrl) || !/^[0-9a-f]{64}$/.test(manifest.assetSha256)) {
  fail("source URL or source archive checksum is not pinned");
}
if (!Array.isArray(manifest.licenses) || manifest.licenses.length === 0) {
  fail("license inventory is missing");
}
if (!existsSync(resolve(VENDOR, "NOTICE.md"))) fail("bundled libmpv notice is missing");
if (manifest.buildScript !== "scripts/release/build-macos-libmpv.mjs") {
  fail(`manifest buildScript is ${manifest.buildScript ?? "missing"}, expected the repository build script`);
}
if (JSON.stringify(builds.ffmpeg.configure) !== JSON.stringify(FFMPEG_CONFIGURE_ARGS)) {
  fail("manifest FFmpeg configure arguments drift from scripts/release/build-macos-libmpv.mjs");
}
if (builds.ffmpeg.deploymentTarget !== MACOSX_DEPLOYMENT_TARGET) {
  fail(
    `manifest deploymentTarget is ${builds.ffmpeg.deploymentTarget ?? "missing"}, ` +
      `expected ${MACOSX_DEPLOYMENT_TARGET}`,
  );
}
if (JSON.stringify(builds.mpv.meson) !== JSON.stringify(MPV_MESON_OPTIONS)) {
  fail("manifest mpv meson options drift from scripts/release/build-macos-libmpv.mjs");
}
if (!Array.isArray(builds.mpv.patches) || builds.mpv.patches.length === 0) {
  fail("manifest does not record the applied mpv patches");
}
const pinnedPatches = (manifest.additionalSources ?? [])
  .filter((entry) => entry.path)
  .map((entry) => entry.path);
if (JSON.stringify(builds.mpv.patches) !== JSON.stringify(pinnedPatches)) {
  fail(
    `manifest builds.mpv.patches [${builds.mpv.patches.join(", ")}] do not match additionalSources ` +
      `[${pinnedPatches.join(", ")}]`,
  );
}
for (const entry of (manifest.additionalSources ?? []).filter((source) => source.path)) {
  if (!/^[0-9a-f]{64}$/.test(String(entry.sha256 ?? ""))) {
    fail(`mpv patch ${entry.path} has no 64-hex SHA-256 pin`);
  }
  const actual = digest(resolve(ROOT, entry.path));
  if (actual !== entry.sha256) {
    fail(`mpv patch ${entry.path} SHA-256 mismatch: expected ${entry.sha256}, got ${actual}`);
  }
}
if (JSON.stringify(builds.mpv.hardDependencies) !== JSON.stringify(MPV_HARD_DEPENDENCIES)) {
  fail(
    `manifest builds.mpv.hardDependencies [${(builds.mpv.hardDependencies ?? []).join(", ")}] ` +
      `do not match [${MPV_HARD_DEPENDENCIES.join(", ")}]`,
  );
}
for (const dependency of MPV_HARD_DEPENDENCIES) {
  if (!manifestFiles.includes(dependency)) fail(`manifest.files is missing hard dependency ${dependency}`);
}
if (manifest.ffmpegBuild !== summarizeFfmpegBuild(builds)) {
  fail("manifest ffmpegBuild is not the summary of builds.ffmpeg");
}
if (!manifest.ffmpegBuild.includes("aresample")) {
  fail("manifest ffmpegBuild does not record the required aresample filter");
}
if (manifest.mpvBuild !== summarizeMpvBuild(builds)) {
  fail("manifest mpvBuild is not the summary of builds.mpv");
}
const ciWorkflow = readFileSync(CI_WORKFLOW, "utf8");
if (!ciWorkflow.includes("pnpm verify:task -- 1.9 1.10") || !ciWorkflow.includes("macos-libmpv-gate-report")) {
  fail("CI does not run the macOS Gate and upload its libmpv report artifact");
}
const releaseWorkflow = readFileSync(RELEASE_WORKFLOW, "utf8");
assertReleaseGateStep(releaseWorkflow);

const contentDigests = [];
for (const [filename, expected] of Object.entries(manifest.files)) {
  const vendor = resolve(VENDOR, filename);
  const bundled = resolve(BUNDLE, "Frameworks", filename);
  if (!existsSync(vendor) || !existsSync(bundled)) fail(`missing ${filename} in vendor or app bundle`);
  if (digest(vendor) !== expected) fail(`vendor checksum mismatch for ${filename}`);
  // Bundling re-signs dylibs, so a raw bundle checksum would differ from the
  // source asset even when its executable content is unchanged. The source
  // checksum remains the supply-chain pin; the signed bundle is bound to that
  // exact vendor input via a re-sign-invariant section-content digest plus
  // LC_ID_DYLIB / LC_BUILD_VERSION / __TEXT geometry equality below. Re-signing
  // only edits header load commands and appends a signature blob after
  // __LINKEDIT — it never changes file-backed section bytes, so the digests
  // stay equal. The bundle is additionally validated via codesign, install
  // name, architecture and the live component probe.
  const binding = assertBundleMatchesVendor(filename);
  contentDigests.push(`${filename} ${binding.bundleContent}`);
  assertRpathInstallName(vendor, filename, `vendor ${filename}`);
  assertRpathInstallName(bundled, filename, `bundle ${filename}`);
  // Exact set equality: "contains arm64" let universal dylibs ship. Check both
  // the release target and the per-file record.
  const actual = parseLipoArchs(run("lipo", ["-archs", bundled]));
  const target = classifyArchitectures(actual, manifest.architectures);
  if (target.status !== "match") {
    fail(
      `${filename} in app bundle is [${actual.join(", ")}], expected release target [${manifest.architectures.join(", ")}]`,
    );
  }
  const declared = manifest.fileArchitectures?.[filename];
  if (!declared) fail(`manifest.fileArchitectures has no entry for ${filename}`);
  const record = classifyArchitectures(actual, declared);
  if (record.status !== "match") {
    fail(
      `${filename} in app bundle is [${actual.join(", ")}], expected fileArchitectures [${declared.join(", ")}]`,
    );
  }
}

const bundleFrameworks = resolve(BUNDLE, "Frameworks");
const bundleFiles = dylibNames(bundleFrameworks);
assertSameSet("bundle Frameworks vs manifest.files", manifestFiles, bundleFiles);
for (const filename of bundleFiles) {
  assertForbiddenLinkage(run("otool", ["-L", resolve(bundleFrameworks, filename)]));
  scannedDylibs += 1;
}
assertHardDependencies(VENDOR, "vendor");
assertHardDependencies(bundleFrameworks, "bundle");
assertDeploymentTargets(VENDOR, "vendor");
assertDeploymentTargets(bundleFrameworks, "bundle");

const appBundle = resolve(BUNDLE, "..");
const main = resolve(BUNDLE, "MacOS", "echo");
const infoPlist = resolve(BUNDLE, "Info.plist");
run("codesign", ["--verify", "--deep", "--strict", appBundle]);
const signature = run("codesign", ["-dv", "--verbose=4", main]);
if (!/Signature=adhoc|Authority=/.test(signature)) fail("Echo executable is unsigned");
const bundleSignature = run("codesign", ["-dv", "--verbose=4", appBundle]);
const codeDirectoryFlags = bundleSignature.match(/flags=[^\s]*\(([^)]*)\)/)?.[1] ?? "";
if (codeDirectoryFlags.split(",").includes("runtime")) {
  fail("ad-hoc app bundle enables hardened runtime and blocks bundled libmpv loading");
}
const bundleIdentifier = run("plutil", [
  "-extract", "CFBundleIdentifier", "raw", "-o", "-", infoPlist,
]).trim();
if (!bundleIdentifier || !bundleSignature.includes(`Identifier=${bundleIdentifier}`)) {
  fail(
    `app bundle signature identifier does not match CFBundleIdentifier (${bundleIdentifier || "missing"})`,
  );
}
if (bundleSignature.includes("Info.plist=not bound")) {
  fail("app bundle signature does not bind Info.plist");
}
const localNetworkUsage = run("plutil", [
  "-extract", "NSLocalNetworkUsageDescription", "raw", "-o", "-", infoPlist,
]).trim();
if (!localNetworkUsage) {
  fail("app bundle is missing NSLocalNetworkUsageDescription");
}
const bonjourServices = run("plutil", [
  "-extract", "NSBonjourServices", "json", "-o", "-", infoPlist,
]);
if (!bonjourServices.includes("_echo-sync._tcp")) {
  fail("app bundle is missing the Echo Bonjour service declaration");
}

const rpaths = run("otool", ["-l", main]);
if (!rpaths.includes("@executable_path/../Frameworks")) fail("Echo rpath does not include bundled Frameworks");

const executableArchitectures = run("lipo", ["-info", main]);
for (const architecture of manifest.architectures) {
  if (!executableArchitectures.includes(architecture)) fail(`Echo executable lacks ${architecture}`);
}

const libmpv = resolve(BUNDLE, "Frameworks", "libmpv.dylib");
const abi = run("otool", ["-L", libmpv]);
if (!abi.includes(`compatibility version ${manifest.libmpvAbi}`)) {
  fail(`libmpv ABI ${manifest.libmpvAbi} is not present`);
}

// The bundled copy must match the vendor input: a rebuild that reintroduces
// video components cannot hide behind a clean manifest.
const bundleProbe = runProbe(bundleFrameworks);
assertProbeMpvVersion(bundleProbe, "bundle");
assertMinimalComponents(bundleProbe, {
  expectedVersion: builds.ffmpeg.version,
  expectedConfigure: FFMPEG_CONFIGURE_ARGS,
  ...(hasMpvOption ? { expectedMpvApiVersion } : {}),
});

const notice = readFileSync(resolve(VENDOR, "NOTICE.md"), "utf8");
const expectedNoticeBlock = renderNoticeBlock(builds.auxiliary);
// Extract the generated block precisely and compare byte-for-byte (enforces the
// BEGIN/END sentinels appear exactly once). An `includes` check can be fooled
// by a duplicate/stale block elsewhere in the file, or by a truncated region.
if (extractNoticeBlock(notice) !== expectedNoticeBlock) {
  fail("NOTICE.md generated auxiliary block does not equal renderNoticeBlock(builds.auxiliary)");
}

const report = [
  "# Echo macOS libmpv Gate report",
  "",
  `- Source: ${manifest.sourceUrl}`,
  `- Archive SHA-256: ${manifest.assetSha256}`,
  `- Architectures: ${manifest.architectures.join(", ")}`,
  `- Deployment target: ${MACOSX_DEPLOYMENT_TARGET}`,
  `- libmpv ABI: ${manifest.libmpvAbi}`,
  `- libmpv build: ${manifest.mpvBuild}`,
  `- mpv client API version: ${expectedMpvApiVersion} (from mpv ${builds.mpv.version}; verified in vendor and bundle probes)`,
  `- Build script: ${manifest.buildScript}`,
  `- Embedded configure: ${bundleProbe.config}`,
  `- Components: ${bundleProbe.decoders.length} decoders / ${bundleProbe.filters.length} filters / ` +
    `${bundleProbe.demuxers.length} demuxers / ${bundleProbe.muxers.length} muxers / ` +
    `${bundleProbe.encoders.length} encoders / protocols in [${bundleProbe.protocolsIn.join(", ")}] ` +
    `out [${bundleProbe.protocolsOut.join(", ")}]`,
  `- Forbidden linkage: none across ${scannedDylibs} dylibs`,
  `- Hard dependencies: ${MPV_HARD_DEPENDENCIES.join(", ")} (present in libmpv closure)`,
  `- Auxiliary: ${builds.auxiliary.map((item) => `${item.component} ${item.version}`).join(", ")}`,
  `- Libraries verified: ${manifestFiles.join(", ")}`,
  `- License components: ${manifest.licenses.map((item) => `${item.component} (${item.license})`).join(", ")}`,
  "- Bundle signature: verified",
  "- Rpath: @executable_path/../Frameworks",
  "- Vendor/bundle binding: re-sign-invariant section-content digest + LC_ID_DYLIB/LC_BUILD_VERSION/__TEXT geometry match",
  "- Bundle section-content digests (sha256):",
  ...contentDigests.map((line) => `  - ${line}`),
  "",
];
await import("node:fs/promises").then(({ writeFile }) => writeFile(REPORT, report.join("\n")));
process.stdout.write(`ok 1.10: macOS libmpv provenance, ABI, rpath, signature and report pass (${REPORT})\n`);
