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
  winNotice: resolve(SRC_TAURI, "vendor", "libmpv", "windows", "NOTICE.md"),
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

// --- 6. Merged license text: coverage + installer wiring ----------------------
check(existsSync(FILES.mergedText), "vendor/libmpv/THIRD-PARTY-LICENSES.txt must exist (merged official license texts shipped in every installer)");
const merged = existsSync(FILES.mergedText) ? read(FILES.mergedText) : "";
// Every license string that appears in any vendor manifest's licenses[] must
// map to a marker in the merged text. Map by family prefix so future entries
// like "GPL-3.0-only" still resolve to the right text block.
const FAMILY_MARKERS = [
  [/^GPL/, "GNU GENERAL PUBLIC LICENSE"],
  [/^LGPL/, "GNU LESSER GENERAL PUBLIC LICENSE"],
  [/^ISC/, "ISC License"],
  [/^MIT/, "MIT License"],
  [/^Apache/, "Apache License"],
  [/^FTL/, "FreeType"],
  [/^Zlib/, "zlib"],
  [/^BSD/, "BSD 3-Clause"],
  [/libpng/i, "PNG Reference Library License version 2"],
];
for (const manifestPath of [FILES.winManifest]) {
  for (const entry of readJson(manifestPath).licenses ?? []) {
    const family = FAMILY_MARKERS.find(([re]) => re.test(entry.license));
    check(
      family !== undefined,
      `license id "${entry.license}" (${entry.component}) has no license-text mapping in task-18.1 — extend FAMILY_MARKERS and THIRD-PARTY-LICENSES.txt`,
    );
    if (family) {
      check(
        merged.includes(family[1]),
        `THIRD-PARTY-LICENSES.txt is missing the full text for "${entry.license}" (marker "${family[1]}")`,
      );
    }
  }
}
// The Linux self-build and macOS NOTICE license sets also feed installers.
for (const [name, marker] of [
  ["LGPL-2.1", "GNU LESSER GENERAL PUBLIC LICENSE"],
  ["FTL", "FreeType"],
  ["MIT", "MIT License"],
  ["Apache-2.0", "Apache License"],
]) {
  check(merged.includes(marker), `THIRD-PARTY-LICENSES.txt is missing the full text covering ${name}`);
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
    Object.values(files).includes("vendor/libmpv/THIRD-PARTY-LICENSES.txt"),
    `tauri.conf.json bundle.linux.${pkg}.files must map THIRD-PARTY-LICENSES.txt into the package (usr/share/doc/echo/)`,
  );
  check(
    Object.values(files).includes("vendor/libmpv/linux/NOTICE.md"),
    `tauri.conf.json bundle.linux.${pkg}.files must carry the Linux NOTICE.md inside the package, not only as a Release attachment`,
  );
}
const winConf = readJson(FILES.tauriWinConf);
check(
  Object.keys(winConf.bundle?.resources ?? {}).includes("vendor/libmpv/THIRD-PARTY-LICENSES.txt"),
  "tauri.windows.conf.json bundle.resources must map vendor/libmpv/THIRD-PARTY-LICENSES.txt into the install root (resources maps source → destination, unlike bundle files maps)",
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
