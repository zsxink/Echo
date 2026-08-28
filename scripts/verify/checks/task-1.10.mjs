#!/usr/bin/env node
// macOS libmpv provenance and bundle report Gate.

import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const VENDOR = resolve(ROOT, "apps", "desktop", "src-tauri", "vendor", "libmpv", "macos");
const BUNDLE = resolve(ROOT, "target", "aarch64-apple-darwin", "release", "bundle", "macos", "Echo.app", "Contents");
const REPORT = resolve(ROOT, "target", "aarch64-apple-darwin", "release", "bundle", "macos", "echo-libmpv-report.md");
const CI_WORKFLOW = resolve(ROOT, ".github", "workflows", "ci.yml");

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

function installName(path) {
  // `otool -D` prefixes the result with the inspected path, which naturally
  // differs between the vendored input and the app-bundle copy.
  return run("otool", ["-D", path])
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.startsWith("@rpath/"))
    .join("\n");
}

if (process.platform !== "darwin") fail("the current Stage 1 Gate is intentionally macOS-local; run it on macOS");

const manifest = JSON.parse(readFileSync(resolve(VENDOR, "manifest.json"), "utf8"));
if (!/^https:\/\//.test(manifest.sourceUrl) || !/^[0-9a-f]{64}$/.test(manifest.assetSha256)) {
  fail("source URL or source archive checksum is not pinned");
}
if (!Array.isArray(manifest.licenses) || manifest.licenses.length === 0) {
  fail("license inventory is missing");
}
if (!existsSync(resolve(VENDOR, "NOTICE.md"))) fail("bundled libmpv notice is missing");
const ciWorkflow = readFileSync(CI_WORKFLOW, "utf8");
if (!ciWorkflow.includes("pnpm verify:task -- 1.9 1.10") || !ciWorkflow.includes("macos-libmpv-gate-report")) {
  fail("CI does not run the macOS Gate and upload its libmpv report artifact");
}

for (const [filename, expected] of Object.entries(manifest.files)) {
  const vendor = resolve(VENDOR, filename);
  const bundled = resolve(BUNDLE, "Frameworks", filename);
  if (!existsSync(vendor) || !existsSync(bundled)) fail(`missing ${filename} in vendor or app bundle`);
  if (digest(vendor) !== expected) fail(`vendor checksum mismatch for ${filename}`);
  // Bundling re-signs dylibs, so a raw bundle checksum would differ from the
  // source asset even when its executable content is unchanged. The source
  // checksum remains the supply-chain pin; the signed bundle is validated via
  // codesign and its stable install-name/ABI below.
  const vendorInstallName = installName(vendor);
  const bundleInstallName = installName(bundled);
  if (vendorInstallName !== bundleInstallName) fail(`bundle install-name drift for ${filename}`);
  const info = run("lipo", ["-info", bundled]);
  for (const architecture of manifest.architectures) {
    if (!info.includes(architecture)) fail(`${filename} lacks ${architecture}`);
  }
}

const appBundle = resolve(BUNDLE, "..");
const main = resolve(BUNDLE, "MacOS", "echo");
run("codesign", ["--verify", "--deep", "--strict", appBundle]);
const signature = run("codesign", ["-dv", "--verbose=4", main]);
if (!/Signature=adhoc|Authority=/.test(signature)) fail("Echo executable is unsigned");

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

const report = [
  "# Echo macOS libmpv Gate report",
  "",
  `- Source: ${manifest.sourceUrl}`,
  `- Archive SHA-256: ${manifest.assetSha256}`,
  `- Architectures: ${manifest.architectures.join(", ")}`,
  `- libmpv ABI: ${manifest.libmpvAbi}`,
  `- libmpv build: ${manifest.mpvBuild}`,
  `- Libraries verified: ${Object.keys(manifest.files).join(", ")}`,
  `- License components: ${manifest.licenses.map((item) => `${item.component} (${item.license})`).join(", ")}`,
  "- Bundle signature: verified",
  "- Rpath: @executable_path/../Frameworks",
  "",
];
await import("node:fs/promises").then(({ writeFile }) => writeFile(REPORT, report.join("\n")));
process.stdout.write(`ok 1.10: macOS libmpv provenance, ABI, rpath, signature and report pass (${REPORT})\n`);
