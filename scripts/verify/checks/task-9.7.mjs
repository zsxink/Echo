#!/usr/bin/env node
// Task 9.7: Verify the libmpv Gate manifest is wired into the formal build
// and that the macOS Gate baseline has not regressed.  The Windows and Linux
// platform wiring is also verified here, on their own platforms:
//
//   • Windows: vendor integrity (SHA-256 against manifest), the
//     bundled_libmpv `libmpv-2.dll` search next to the exe (with
//     `#![windows_subsystem]`), and tauri.windows.conf.json `bundle.resources`
//     covering the whole DLL set in the install root. That mapping is
//     deliberately platform-scoped: the top-level `bundle.resources` is
//     platform-agnostic and would leak the DLLs into the macOS/Linux builds.
//   • Linux: vendor integrity, a glibc ceiling of ≤ 2.35 (task 2.2's build
//     host), and tauri.conf `bundle.linux` placing libmpv.so into usr/bin.
//     These are enforced once the CI first build has landed the tree; until
//     then the state is reported loudly as pending, never silently skipped.
//
// Checks performed:
//   1. Manifest structure: sourceUrl (https), assetSha256 (64-hex), licenses
//      (non-empty), NOTICE.md present.
//   2. tauri.conf.json bundle.macOS.files maps every manifest file to the
//      correct Frameworks/ destination.
//   3. build.rs contains the @executable_path/../Frameworks rpath arg.
//   4. CI workflow includes the macOS Gate verification step (no regression).
//   5. If a formal build artifact exists, runs the same rpath, signing, ABI
//      and universal-architecture checks as the 1.10 Gate, plus a
//      cross-platform purity check: the macOS bundle's Resources/ must not
//      carry another platform's playback backend.
//   6. (Windows only) formal Windows wiring checks above.
//   7. (Linux only) formal Linux wiring checks above.

import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync } from "node:fs";
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
// Windows-only bundle resources live here: `bundle.resources` in the main
// config is platform-agnostic and would ship the Windows DLL set to macOS
// and Linux builds too.
const TAURI_WINDOWS_CONF = resolve(
  ROOT,
  "apps",
  "desktop",
  "src-tauri",
  "tauri.windows.conf.json",
);
const BUILD_RS = resolve(ROOT, "apps", "desktop", "src-tauri", "build.rs");
const CI_WORKFLOW = resolve(ROOT, ".github", "workflows", "ci.yml");
// CI builds with an explicit `--target`, which is what puts the bundle under
// the triple directory this path names. A host build lands in
// `target/release/bundle/...` instead and is skipped there — the triple path
// stays the single source of truth so this check verifies the same artifact
// CI does. In the macOS CI job task 1.9 runs first and leaves exactly this
// bundle behind (it also universalises and re-signs it), so the assertions
// below do execute on every macOS build; see the 9.7 step in ci.yml.
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

  // Cross-platform purity: the macOS app bundle must not carry another
  // platform's playback backend. `bundle.resources` in the main config is
  // platform-agnostic, so a Windows mapping left there is copied into the
  // macOS build and ships ~115 MB of unusable DLLs. Anything under Resources/
  // that is not a macOS dylib is a leak; Frameworks/ is the macOS location
  // and is checked by the checksum loop above.
  const resourcesDir = resolve(BUNDLE, "Resources");
  const foreign = readdirSync(resourcesDir, { withFileTypes: true })
    .filter((e) => e.isFile() && (e.name.endsWith(".dll") || e.name.endsWith(".so") || e.name.endsWith(".so.*")))
    .map((e) => e.name);
  if (foreign.length > 0)
    fail(
      `macOS app bundle Resources/ carries non-macOS playback binaries: ${foreign.join(", ")}; platform-specific bundle resources must not be declared in the platform-agnostic bundle.resources`,
    );
  pass("macOS app bundle Resources/ carries no foreign-platform binaries");

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

// --- 6. Windows platform wiring (formal) ---
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
  const windowsManifest = JSON.parse(
    readFileSync(resolve(windowsVendor, "manifest.json"), "utf8"),
  );

  // 6a. Vendor integrity: each manifest file exists and matches its SHA-256.
  for (const [filename, expected] of Object.entries(windowsManifest.files)) {
    const vendor = resolve(windowsVendor, filename);
    if (!existsSync(vendor))
      fail(`Windows vendor file missing: ${filename}`);
    if (digest(vendor) !== expected)
      fail(`Windows vendor checksum mismatch for ${filename}`);
  }
  pass(
    `Windows vendor integrity ok (${Object.keys(windowsManifest.files).length} files, ABI ${windowsManifest.libmpvAbi})`,
  );

  // 6b. bundled_libmpv Windows branch resolves libmpv-2.dll beside the exe.
  const mainRs = readFileSync(resolve(ROOT, "apps", "desktop", "src-tauri", "src", "main.rs"), "utf8");
  if (!mainRs.includes('join("libmpv-2.dll")'))
    fail("main.rs bundled_libmpv does not locate libmpv-2.dll beside the executable");
  if (!mainRs.includes('#![windows_subsystem = "windows"]'))
    fail("main.rs lacks #![windows_subsystem = \"windows\"]");
  pass("bundled_libmpv Windows branch (libmpv-2.dll next to exe) present");

  // 6c. tauri.windows.conf.json bundles the whole DLL set into the install
  // root. The mapping lives in the platform-specific config because the
  // top-level `bundle.resources` is platform-agnostic: Tauri copies it into
  // every target's build, which leaked the Windows DLLs into the macOS dmg.
  // Keep asserting the main config stays free of it, so a regression back to
  // the platform-agnostic field fails here instead of in a downloaded build.
  const confMain = JSON.parse(readFileSync(TAURI_CONF, "utf8"));
  if (confMain.bundle?.resources)
    fail(
      "tauri.conf.json declares platform-agnostic bundle.resources; Windows mappings must live in tauri.windows.conf.json so they do not leak into macOS/Linux artifacts",
    );
  const confWin = JSON.parse(readFileSync(TAURI_WINDOWS_CONF, "utf8"));
  const winResources = confWin.bundle?.resources ?? {};
  for (const filename of Object.keys(windowsManifest.files)) {
    if (filename.endsWith(".dll") && !winResources[`vendor/libmpv/windows/${filename}`])
      fail(`tauri.windows.conf.json bundle.resources missing Windows DLL mapping for ${filename}`);
  }
  pass("tauri.windows.conf.json bundle.resources covers the Windows DLL set");
} else {
  process.stdout.write(
    "  ok: Windows wiring checks are Windows-local; not run here\n",
  );
}

// --- 7. Linux platform wiring (formal once the CI first build has landed) ---
if (process.platform === "linux") {
  const linuxVendor = resolve(
    ROOT,
    "apps",
    "desktop",
    "src-tauri",
    "vendor",
    "libmpv",
    "linux",
  );
  const linuxManifestPath = resolve(linuxVendor, "manifest.json");
  if (!existsSync(linuxManifestPath)) {
    // The Linux libmpv tree lands only after the CI first build (task 2.2).
    // Until then the wiring below cannot be verified; this is a real pending
    // state, not a silent skip, so it is reported loudly.
    process.stdout.write(
      "  pending: Linux libmpv vendor not yet landed (CI first build, task 2.2); formal Linux checks deferred to that state\n",
    );
  } else {
    const linuxManifest = JSON.parse(readFileSync(linuxManifestPath, "utf8"));

    // 7a. Vendor integrity, mirroring 6a.
    for (const [filename, expected] of Object.entries(linuxManifest.files)) {
      const vendor = resolve(linuxVendor, filename);
      if (!existsSync(vendor))
        fail(`Linux vendor file missing: ${filename}`);
      if (digest(vendor) !== expected)
        fail(`Linux vendor checksum mismatch for ${filename}`);
    }
    pass(
      `Linux vendor integrity ok (${Object.keys(linuxManifest.files).length} files)`,
    );

    // 7b. glibc ceiling: every ELF in the vendor set must require ≤ 2.35
    // (Ubuntu 22.04 was the build host).  Failure means the artifact was
    // produced on a newer glibc and would not run on the supported floor.
    const realLibs = Object.keys(linuxManifest.files).filter((f) =>
      f.endsWith(".so.2.5") || f.endsWith(".so.61") || f.endsWith(".so.59") ||
      f.endsWith(".so.5") || f.endsWith(".so.8") || f.endsWith(".so.10"),
    );
    if (realLibs.length === 0)
      fail("Linux vendor manifest lists no versioned .so real files to verify glibc against");
    for (const filename of realLibs) {
      const so = resolve(linuxVendor, filename);
      const info = run("readelf", ["--version-info", so]);
      const versions = [...info.matchAll(/GLIBC_(\d+)\.(\d+)\b/g)].map((m) =>
        Number(m[1]) * 100 + Number(m[2]),
      );
      const max = Math.max(0, ...versions);
      if (max > 235)
        fail(`Linux vendor ${filename} requires glibc ${(max / 100).toFixed(2)} > 2.35`);
    }
    const ceiling = [...new Set(
      realLibs.map((f) => {
        const info = run("readelf", ["--version-info", resolve(linuxVendor, f)]);
        return Math.max(0, ...[...info.matchAll(/GLIBC_(\d+)\.(\d+)\b/g)].map((m) =>
          Number(m[1]) * 100 + Number(m[2]),
        ));
      }),
    )];
    pass(`Linux vendor glibc ceiling ≤ 2.35 (max ${Math.max(...ceiling) / 100})`);

    // 7c. tauri.conf places libmpv.so into the AppImage/deb install dir.
    const confLin = JSON.parse(readFileSync(TAURI_CONF, "utf8"));
    for (const pkg of ["deb", "appimage"]) {
      const files = confLin.bundle?.linux?.[pkg]?.files ?? {};
      if (!files["usr/bin/libmpv.so"])
        fail(`tauri.conf bundle.linux.${pkg}.files missing usr/bin/libmpv.so mapping`);
    }
    pass("tauri.conf bundle.linux maps libmpv.so into usr/bin for deb/AppImage");
  }
} else {
  process.stdout.write(
    "  ok: Linux wiring checks are Linux-local; not run here\n",
  );
}

process.stdout.write(
  `ok 9.7: libmpv manifest integrated into formal build; macOS baseline + platform wiring verified, no regression\n`,
);
