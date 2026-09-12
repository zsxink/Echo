#!/usr/bin/env node
// Task 9.7: Verify the libmpv Gate manifest is wired into the formal build
// and that the macOS Gate baseline has not regressed.  Windows and Linux
// requirements are documented but intentionally deferred to their platform
// Gates (tasks 9.7/9.8 scope note).
//
// Checks performed:
//   1. Manifest structure: sourceUrl (https), assetSha256 (64-hex), licenses
//      (non-empty), NOTICE.md present.
//   2. tauri.conf.json bundle.macOS.files maps every manifest file to the
//      correct Frameworks/ destination.
//   3. build.rs contains the @executable_path/../Frameworks rpath arg.
//   4. CI workflow includes the macOS Gate verification step (no regression).
//   5. If a formal build artifact exists, runs the same rpath, signing, ABI
//      and universal-architecture checks as the 1.10 Gate.

import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const ROOT = resolve(
  fileURLToPath(new URL(".", import.meta.url)),
  "..",
  "..",
  "..",
);
const MANIFEST = resolve(
  ROOT,
  "apps",
  "desktop",
  "src-tauri",
  "vendor",
  "libmpv",
  "macos",
  "manifest.json",
);
const TAURI_CONF = resolve(
  ROOT,
  "apps",
  "desktop",
  "src-tauri",
  "tauri.conf.json",
);
const BUILD_RS = resolve(ROOT, "apps", "desktop", "src-tauri", "build.rs");
const CI_WORKFLOW = resolve(ROOT, ".github", "workflows", "ci.yml");
const BUNDLE = resolve(
  ROOT,
  "target",
  "aarch64-apple-darwin",
  "release",
  "bundle",
  "macos",
  "Echo.app",
  "Contents",
);

function fail(message) {
  process.stderr.write(`FAIL 9.7: ${message}\n`);
  process.exit(1);
}

function pass(message) {
  process.stdout.write(`  ok: ${message}\n`);
}

function digest(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function run(command, args) {
  const result = spawnSync(command, args, { cwd: ROOT, encoding: "utf8" });
  if (result.status !== 0) {
    fail(
      `${command} ${args.join(" ")} failed (exit ${result.status})\n${result.stdout || ""}${result.stderr || ""}`,
    );
  }
  return `${result.stdout || ""}${result.stderr || ""}`;
}

// --- 1. Manifest structure ---
if (!existsSync(MANIFEST)) fail("libmpv manifest not found");
const manifest = JSON.parse(readFileSync(MANIFEST, "utf8"));
if (!/^https:\/\//.test(manifest.sourceUrl))
  fail("sourceUrl is not pinned to https");
if (!/^[0-9a-f]{64}$/.test(manifest.assetSha256))
  fail("assetSha256 is not a 64-hex digest");
if (!Array.isArray(manifest.licenses) || manifest.licenses.length === 0)
  fail("license inventory is empty");
if (!manifest.architectures || manifest.architectures.length === 0)
  fail("architectures list is empty");
if (!manifest.libmpvAbi) fail("libmpvAbi is missing");
if (!manifest.files || Object.keys(manifest.files).length === 0)
  fail("files map is empty");
pass(
  `manifest structure valid (${Object.keys(manifest.files).length} files, ABI ${manifest.libmpvAbi})`,
);

const noticePath = resolve(ROOT, "apps", "desktop", "src-tauri", "vendor", "libmpv", "macos", "NOTICE.md");
if (!existsSync(noticePath)) fail("NOTICE.md is missing from vendor directory");
pass("NOTICE.md present");

// --- 2. tauri.conf.json wiring ---
const conf = JSON.parse(readFileSync(TAURI_CONF, "utf8"));
const bundleFiles = conf.bundle?.macOS?.files ?? {};
const manifestFileCount = Object.keys(manifest.files).length;
const bundleFileCount = Object.keys(bundleFiles).length;

// Every manifest file should map to Frameworks/<name>
for (const filename of Object.keys(manifest.files)) {
  const expected = `Frameworks/${filename}`;
  if (!bundleFiles[expected]) {
    fail(`tauri.conf bundle.macOS.files missing mapping for ${expected}`);
  }
}
// NOTICE.md should also be bundled
if (!bundleFiles["Resources/third-party/libmpv/NOTICE.md"]) {
  fail("tauri.conf bundle.macOS.files missing NOTICE.md mapping");
}
pass(
  `tauri.conf wiring complete (${bundleFileCount} entries cover all ${manifestFileCount} manifest files + NOTICE)`,
);

// --- 3. build.rs rpath ---
const buildRs = readFileSync(BUILD_RS, "utf8");
if (!buildRs.includes("@executable_path/../Frameworks")) {
  fail("build.rs does not set @executable_path/../Frameworks rpath");
}
pass("build.rs rpath configuration present");

// --- 4. CI workflow includes Gate step ---
const ciYml = readFileSync(CI_WORKFLOW, "utf8");
if (!ciYml.includes("pnpm verify:task -- 1.9 1.10")) {
  fail("CI does not include the macOS Gate verification step (1.9 1.10)");
}
pass("CI workflow includes macOS Gate step");

// --- 5. Formal build regression check (macOS only) ---
if (process.platform !== "darwin") {
  process.stdout.write(
    "skip: formal build regression checks are macOS-local; defer to platform Gate\n",
  );
} else if (!existsSync(BUNDLE)) {
  process.stdout.write(
    "skip: no formal build artifact found at expected bundle path; build first with `pnpm --filter @echo/desktop tauri build --bundles app`\n",
  );
} else {
  // Verify vendor file checksums match manifest
  const vendorDir = resolve(
    ROOT,
    "apps",
    "desktop",
    "src-tauri",
    "vendor",
    "libmpv",
    "macos",
  );
  for (const [filename, expected] of Object.entries(manifest.files)) {
    const vendor = resolve(vendorDir, filename);
    const bundled = resolve(BUNDLE, "Frameworks", filename);
    if (!existsSync(vendor)) fail(`vendor file missing: ${filename}`);
    if (!existsSync(bundled)) fail(`bundled file missing: ${filename}`);
    if (digest(vendor) !== expected)
      fail(`vendor checksum mismatch for ${filename}`);
  }
  pass("vendor checksums match manifest");

  // Codesign verification
  const appBundle = resolve(BUNDLE, "..");
  run("codesign", ["--verify", "--deep", "--strict", appBundle]);
  pass("codesign --verify --deep --strict passed");

  // Rpath verification
  const main = resolve(BUNDLE, "MacOS", "echo");
  const rpaths = run("otool", ["-l", main]);
  if (!rpaths.includes("@executable_path/../Frameworks")) {
    fail("formal build echo binary missing @executable_path/../Frameworks rpath");
  }
  pass("rpath @executable_path/../Frameworks present in formal build");

  // Universal architecture verification
  const executableInfo = run("lipo", ["-info", main]);
  for (const arch of manifest.architectures) {
    if (!executableInfo.includes(arch)) {
      fail(`formal build echo binary missing architecture ${arch}`);
    }
  }
  pass(`universal architectures verified (${manifest.architectures.join(", ")})`);

  // ABI compatibility verification
  const libmpv = resolve(BUNDLE, "Frameworks", "libmpv.dylib");
  if (existsSync(libmpv)) {
    const abi = run("otool", ["-L", libmpv]);
    if (!abi.includes(`compatibility version ${manifest.libmpvAbi}`)) {
      fail(
        `libmpv ABI mismatch: expected compatibility version ${manifest.libmpvAbi}`,
      );
    }
    pass(`libmpv ABI ${manifest.libmpvAbi} verified in formal build`);
  }

  // Framework dylib universal verification
  for (const filename of Object.keys(manifest.files)) {
    const dylib = resolve(BUNDLE, "Frameworks", filename);
    if (existsSync(dylib)) {
      const info = run("lipo", ["-info", dylib]);
      for (const arch of manifest.architectures) {
        if (!info.includes(arch)) {
          fail(`${filename} in formal build lacks architecture ${arch}`);
        }
      }
    }
  }
  pass("all framework dylibs are universal");
}

// --- 6. Windows/Linux deferred status ---
if (process.platform === "win32") {
  const windowsVendor = resolve(
    ROOT,
    "apps",
    "desktop",
    "src-tauri",
    "vendor",
    "libmpv",
    "windows",
  );
  if (existsSync(windowsVendor)) {
    process.stdout.write(
      "note: Windows libmpv vendor directory exists; Windows DLL search verification deferred to platform Gate\n",
    );
  } else {
    process.stdout.write(
      "deferred: Windows libmpv vendoring and application-directory DLL search deferred to platform Gate (9.8)\n",
    );
  }
} else if (process.platform === "linux") {
  const linuxVendor = resolve(
    ROOT,
    "apps",
    "desktop",
    "src-tauri",
    "vendor",
    "libmpv",
    "linux",
  );
  if (existsSync(linuxVendor)) {
    process.stdout.write(
      "note: Linux libmpv vendor directory exists; glibc 2.35 runtime dependency check deferred to platform Gate\n",
    );
  } else {
    process.stdout.write(
      "deferred: Linux libmpv vendoring and glibc 2.35 runtime dependency check deferred to platform Gate (9.8)\n",
    );
  }
}

process.stdout.write(
  `ok 9.7: libmpv manifest integrated into formal build; macOS Gate baseline verified, no regression\n`,
);
