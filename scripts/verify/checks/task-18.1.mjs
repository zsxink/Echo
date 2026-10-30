#!/usr/bin/env node
// Licensing & privacy notices contract gate (task 18.1, issue #66).
//
// Issue #66 found four documentation-level legal risks; the fix is entirely
// text/metadata — a rewritten custom LICENSE, an SPDX LicenseRef in
// Cargo.toml, per-platform-correct libmpv license records, a merged license
// text packaged into all three installers, and media-rights/privacy docs.
// None of that can fail at compile time, so this static gate makes regressions
// loud. It asserts, mirroring the task-17.1 style:
//
//   1. Cargo SPDX: the workspace license is the LicenseRef custom license and
//      nothing labels Echo as standard BUSL-1.1 (the BUSL base grant includes
//      redistribution rights; Echo's license deliberately does not — the two
//      are not interchangeable).
//   2. LICENSE: written as Echo Personal Study License, not the old
//      grant-then-restrict BUSL-structured text.
//   3. LICENSES.md: same naming, per-platform libmpv note.
//   4. Per-platform player-binary license records match the actual build
//      configs: Windows (shinchiro --enable-gpl + x264/x265) and the Linux
//      self-build (--enable-gpl kept) are GPL-2.0-or-later, macOS stays LGPL.
//   5. NOTICE files point at corresponding source.
//   6. THIRD-PARTY-LICENSES.txt covers every license family referenced by the
//      three vendor manifests and is wired into all three installers.
//   7. Both READMEs carry the media-rights & privacy section and the
//      Personal-Study naming; docs/media-rights-and-privacy.md keeps the
//      legal-review release gate.
//
// Usage:
//   node scripts/verify/checks/task-18.1.mjs
//
// Exits 0 when every contract item holds, 1 with all violations printed.

import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const SRC_TAURI = resolve(ROOT, "apps", "desktop", "src-tauri");
const FILES = {
  cargoToml: resolve(ROOT, "Cargo.toml"),
  license: resolve(ROOT, "LICENSE"),
  licensesMd: resolve(ROOT, "LICENSES.md"),
  denyToml: resolve(ROOT, "deny.toml"),
  tauriConf: resolve(SRC_TAURI, "tauri.conf.json"),
  tauriWinConf: resolve(SRC_TAURI, "tauri.windows.conf.json"),
  mergedText: resolve(SRC_TAURI, "vendor", "libmpv", "THIRD-PARTY-LICENSES.txt"),
  winManifest: resolve(SRC_TAURI, "vendor", "libmpv", "windows", "manifest.json"),
  macosManifest: resolve(SRC_TAURI, "vendor", "libmpv", "macos", "manifest.json"),
  winNotice: resolve(SRC_TAURI, "vendor", "libmpv", "windows", "NOTICE.md"),
  winBuildInfo: resolve(SRC_TAURI, "vendor", "libmpv", "windows", "BUILD_INFO.md"),
  macNotice: resolve(SRC_TAURI, "vendor", "libmpv", "macos", "NOTICE.md"),
  linuxScript: resolve(ROOT, "scripts", "release", "build-linux-libmpv.mjs"),
  readmeZh: resolve(ROOT, "README.md"),
  readmeEn: resolve(ROOT, "README.en.md"),
  mediaDoc: resolve(ROOT, "docs", "media-rights-and-privacy.md"),
};

const violations = [];
const check = (ok, message) => {
  if (!ok) violations.push(message);
};
const read = (p) => readFileSync(p, "utf8");
const readJson = (p) => JSON.parse(read(p));

// --- 1. Cargo SPDX + repo-wide BUSL-1.1 labeling -----------------------------
const cargo = read(FILES.cargoToml);
check(
  cargo.includes('license = "LicenseRef-Echo-Personal-Study-1.0"'),
  'Cargo.toml workspace license must be "LicenseRef-Echo-Personal-Study-1.0" — Echo Personal Study License is an independent custom license, not standard BUSL-1.1 (the BUSL base grant includes redistribution; Echo\'s does not), and custom licenses are expressed via LicenseRef',
);
check(
  !/^\s*license\s*=\s*"BUSL-1\.1"/m.test(cargo) && !/\| `?echo-\w+`? \| [^|]*BUSL-1\.1/.test(read(FILES.licensesMd)),
  "no manifest or LICENSES.md table row may label an Echo workspace crate as BUSL-1.1",
);
const deny = read(FILES.denyToml);
check(
  deny.includes("LicenseRef-Echo-Personal-Study-1.0"),
  "deny.toml must document the LicenseRef private-license exemption (its [licenses.private] ignore covers Echo's own license)",
);

// --- 2. LICENSE shape ---------------------------------------------------------
check(existsSync(FILES.license), "root LICENSE must exist");
const license = read(FILES.license);
check(
  license.includes("Echo 个人学习使用许可") && license.includes("Echo Personal Study License"),
  'LICENSE must be written as "Echo 个人学习使用许可 / Echo Personal Study License" (issue #66: the old BUSL-1.1-titled text granted redistribution in its terms and forbade it in its additional restrictions)',
);
check(
  !license.includes("以本附加限制为准"),
  'LICENSE must not contain the old "附加限制优先" override clause — the rewritten grant enumerates rights explicitly instead of granting then revoking',
);
check(
  license.includes("GPL-2.0-or-later") && license.includes("2099"),
  "LICENSE must keep the Change Date (2099-12-31) → GPL-2.0-or-later conversion",
);
check(
  license.includes("第三方组件"),
  "LICENSE must keep third-party components under their upstream licenses (Echo's own restrictions must not limit GPL/LGPL redistribution rights for bundled player binaries)",
);
check(
  license.includes("中华人民共和国法律"),
  "LICENSE must state governing law and dispute forum",
);

// --- 3. LICENSES.md naming + per-platform note -------------------------------
const licensesMd = read(FILES.licensesMd);
check(
  licensesMd.includes("LicenseRef-Echo-Personal-Study-1.0") && licensesMd.includes("Echo Personal Study License 1.0"),
  "LICENSES.md must name the custom license (Echo Personal Study License 1.0 / LicenseRef-Echo-Personal-Study-1.0) in its intro and workspace table",
);
check(
  licensesMd.includes("GPL-2.0-or-later") && licensesMd.includes("macOS"),
  "LICENSES.md's libmpv note must state per-platform licensing (macOS LGPL self-build; Windows/Linux GPL)",
);

// --- 4. Per-platform player-binary license records ---------------------------
const winManifest = readJson(FILES.winManifest);
for (const entry of winManifest.licenses ?? []) {
  if (/mpv|FFmpeg/i.test(entry.component)) {
    check(
      entry.license === "GPL-2.0-or-later",
      `Windows manifest component "${entry.component}" must be GPL-2.0-or-later — the shinchiro build configures FFmpeg with --enable-gpl and links x264/x265 (packages/ffmpeg.cmake), making the static-FFmpeg libmpv-2.dll GPL, not the previously recorded LGPL`,
    );
  }
}
const linuxScript = read(FILES.linuxScript);
check(
  linuxScript.includes('"FFmpeg", "GPL-2.0-or-later"'),
  'build-linux-libmpv.mjs LICENSES table must record FFmpeg as GPL-2.0-or-later — the patched ffmpeg-config keeps --enable-gpl, which marks the FFmpeg build GPL',
);
const macNotice = read(FILES.macNotice);
check(
  macNotice.includes("Corresponding source") && macNotice.includes("THIRD-PARTY-LICENSES.txt"),
  "macOS NOTICE must carry a Corresponding-source section and point at the packaged license texts",
);

// --- 5. Windows NOTICE license + source sections ------------------------------
const winNotice = read(FILES.winNotice);
check(
  winNotice.includes("GPL-2.0-or-later") && winNotice.includes("## Licenses"),
  "Windows NOTICE must contain a Licenses section recording GPL-2.0-or-later for the bundled binaries",
);
check(
  winNotice.includes("Corresponding source"),
  "Windows NOTICE must state how to obtain the corresponding source (shinchiro build scripts + upstream repos)",
);
// BUILD_INFO.md is the Windows provenance table shown to auditors; it used to
// carry the pre-issue-#66 LGPL claim and contradicted manifest.json/NOTICE.
check(existsSync(FILES.winBuildInfo), "vendor/libmpv/windows/BUILD_INFO.md must exist");
const winBuildInfo = existsSync(FILES.winBuildInfo) ? read(FILES.winBuildInfo) : "";
check(
  winBuildInfo.includes("GPL-2.0-or-later") && !winBuildInfo.includes("LGPL-2.1"),
  "Windows BUILD_INFO.md must record the bundled libmpv-2.dll as GPL-2.0-or-later with no LGPL-2.1 license claim (issue #66: shinchiro --enable-gpl + x264/x265 makes the static-FFmpeg DLL GPL)",
);

// --- 6. Merged license text: coverage + installer wiring ----------------------
check(existsSync(FILES.mergedText), "vendor/libmpv/THIRD-PARTY-LICENSES.txt must exist (merged official license texts shipped in every installer)");
const merged = existsSync(FILES.mergedText) ? read(FILES.mergedText) : "";
// Every license string that appears in any vendor manifest's licenses[] (or
// the Linux build script's LICENSES table) must map to an SPDX section in the
// merged text, and that section must carry real body lines — not just a
// header. The mapping is exact on the SPDX id prefix, so a future GPL-3.0-only
// or BSD-2-Clause entry fails the gate until its full text is added, instead
// of false-passing onto a different version's text block.
const SPDX_SECTIONS = [
  [/^GPL-2\.0/, "GPL-2.0"],
  [/^LGPL-2\.1/, "LGPL-2.1"],
  [/^ISC/, "ISC"],
  [/^MIT/, "MIT"],
  [/^Apache-2\.0/, "Apache-2.0"],
  [/^FTL/, "FTL"],
  [/^Zlib/, "Zlib"],
  [/^BSD-3-Clause/, "BSD-3-Clause"],
  [/^libpng/i, "libpng-2.0"],
];
const SECTION_MIN_LINES = {
  "GPL-2.0": 300,
  "LGPL-2.1": 400,
  "Apache-2.0": 150,
  "FTL": 60,
  "libpng-2.0": 20,
  "MIT": 4,
  "ISC": 4,
  "Zlib": 4,
  "BSD-3-Clause": 4,
};
function checkLicenseTextCoverage(license, origin) {
  const section = SPDX_SECTIONS.find(([re]) => re.test(license));
  check(
    section !== undefined,
    `license id "${license}" (${origin}) has no SPDX section mapping in task-18.1 — extend SPDX_SECTIONS and add the full text to THIRD-PARTY-LICENSES.txt`,
  );
  if (!section) return;
  const id = section[1];
  const marker = `== SPDX: ${id}`;
  const start = merged.indexOf(marker);
  check(
    start !== -1,
    `THIRD-PARTY-LICENSES.txt is missing the "${marker}" section (needed by ${origin}: "${license}")`,
  );
  if (start === -1) return;
  const next = merged.indexOf("\n== SPDX: ", start + marker.length);
  const body = merged.slice(start, next === -1 ? merged.length : next);
  const lineCount = body.split("\n").length;
  check(
    lineCount >= SECTION_MIN_LINES[id],
    `THIRD-PARTY-LICENSES.txt section ${id} has only ${lineCount} lines (minimum ${SECTION_MIN_LINES[id]}) — the full license text may have been truncated`,
  );
}
for (const entry of readJson(FILES.winManifest).licenses ?? []) {
  checkLicenseTextCoverage(entry.license, `Windows manifest: ${entry.component}`);
}
for (const entry of readJson(FILES.macosManifest).licenses ?? []) {
  checkLicenseTextCoverage(entry.license, `macOS manifest: ${entry.component}`);
}
// The Linux vendor manifest is generated at release time and never sits in
// the repository, so the Linux license set is checked through its generator:
// every 3-string row of build-linux-libmpv.mjs's LICENSES table. Match inside
// the `const LICENSES = [...]` block only — the script has other 3-string
// arrays (e.g. spawnSync stdio: ["ignore", "pipe", "ignore"]) that must not
// be mistaken for license rows.
const licensesBlock = linuxScript.match(/const LICENSES = \[([\s\S]*?)\];/);
check(licensesBlock !== undefined, "build-linux-libmpv.mjs must define a const LICENSES table");
const linuxLicenseRows = licensesBlock
  ? [...licensesBlock[1].matchAll(/\["([^"]+)",\s*"([^"]+)",\s*"[^"]*"\]/g)]
  : [];
check(
  linuxLicenseRows.length >= 8,
  `build-linux-libmpv.mjs LICENSES table should carry every component row (found ${linuxLicenseRows.length})`,
);
for (const [, component, license] of linuxLicenseRows) {
  checkLicenseTextCoverage(license, `Linux build script: ${component}`);
}

const tauriConf = readJson(FILES.tauriConf);
const macFiles = tauriConf.bundle?.macOS?.files ?? {};
check(
  macFiles["Resources/third-party/libmpv/THIRD-PARTY-LICENSES.txt"] === "vendor/libmpv/THIRD-PARTY-LICENSES.txt",
  "tauri.conf.json bundle.macOS.files must map THIRD-PARTY-LICENSES.txt into Resources/third-party/libmpv/",
);
for (const pkg of ["deb", "appimage"]) {
  const files = tauriConf.bundle?.linux?.[pkg]?.files ?? {};
  check(
    files["usr/share/doc/echo/THIRD-PARTY-LICENSES.txt"] === "vendor/libmpv/THIRD-PARTY-LICENSES.txt",
    `tauri.conf.json bundle.linux.${pkg}.files must map vendor/libmpv/THIRD-PARTY-LICENSES.txt exactly to usr/share/doc/echo/THIRD-PARTY-LICENSES.txt (bundle files maps destination → source)`,
  );
  check(
    files["usr/share/doc/echo/NOTICE-libmpv-linux.md"] === "vendor/libmpv/linux/NOTICE.md",
    `tauri.conf.json bundle.linux.${pkg}.files must carry the Linux NOTICE.md inside the package (usr/share/doc/echo/NOTICE-libmpv-linux.md), not only as a Release attachment`,
  );
}
const winConf = readJson(FILES.tauriWinConf);
check(
  winConf.bundle?.resources?.["vendor/libmpv/THIRD-PARTY-LICENSES.txt"] === "THIRD-PARTY-LICENSES.txt",
  "tauri.windows.conf.json bundle.resources must map vendor/libmpv/THIRD-PARTY-LICENSES.txt exactly to THIRD-PARTY-LICENSES.txt in the install root (resources maps source → destination, unlike bundle files maps)",
);

// --- 7. Media rights & privacy docs ------------------------------------------
const mediaDoc = existsSync(FILES.mediaDoc) ? read(FILES.mediaDoc) : "";
check(existsSync(FILES.mediaDoc), "docs/media-rights-and-privacy.md must exist (user media rights + privacy boundary, issue #66)");
check(
  mediaDoc.includes("律师") && mediaDoc.includes("不得发布"),
  "media-rights doc must keep the pre-release legal-review gate (lawyer review before public release)",
);
check(
  mediaDoc.includes("必须先更新本文档") || mediaDoc.includes("must be updated before release"),
  "media-rights doc must keep the future-features obligation (privacy notice before any data-egress feature ships)",
);
for (const [name, path, mustHave] of [
  ["README.md", FILES.readmeZh, "个人学习使用许可"],
  ["README.en.md", FILES.readmeEn, "Echo Personal Study License"],
]) {
  const content = read(path);
  check(
    content.includes(mustHave),
    `${name} license section must name the Echo Personal Study License instead of a BUSL-1.1-based description`,
  );
  check(
    content.includes("media-rights-and-privacy.md"),
    `${name} must link the media-rights & privacy section (docs/media-rights-and-privacy.md)`,
  );
}
check(
  !read(FILES.readmeZh).includes("基于 Business Source License 1.1 的定制授权") &&
    !read(FILES.readmeEn).includes("custom license based on the Business Source License 1.1"),
  "READMEs must not describe Echo's license as BUSL-1.1-based (issue #66: that description contradicts the no-redistribution grant)",
);

// ---------------------------------------------------------------------------
if (violations.length > 0) {
  process.stderr.write(`FAIL 18.1: ${violations.length} violation(s)\n`);
  for (const v of violations) process.stderr.write(`  - ${v}\n`);
  process.exit(1);
}
process.stdout.write("ok 18.1: licensing & privacy notices contract holds (issue #66)\n");
