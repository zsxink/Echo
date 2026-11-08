#!/usr/bin/env node
// Regenerate the repository's third-party license inventory and the notice
// file shipped inside every desktop installer.
//
// Inputs (single sources of truth):
//   * `cargo metadata --format-version 1` — the full Cargo.lock graph. This is
//     a conservative superset (it also contains dev- and target-specific
//     crates); over-listing a dependency cannot create a compliance gap.
//   * `apps/desktop/package.json` + the `apps/desktop` importer of
//     `pnpm-lock.yaml` — production npm dependencies only. npm license
//     identifiers are not present in the lockfile, so they are recorded in
//     NPM_LICENSE_METADATA below and cross-checked against the installed
//     package.json whenever node_modules is available.
//
// Outputs:
//   * LICENSES.md — human-readable inventory at the repository root.
//   * apps/desktop/src-tauri/vendor/libmpv/THIRD-PARTY-NOTICES.txt —
//     dependency inventory mapped to the `== SPDX: <id>` sections of
//     THIRD-PARTY-LICENSES.txt, wired into all three installers.
//
// Usage:
//   node scripts/release/generate-third-party-notices.mjs            # write
//   node scripts/release/generate-third-party-notices.mjs --check    # verify
//
// `--check` never writes; it recomputes both outputs in memory and exits 1
// with a short diff when the committed files are stale. task-18.1 runs it so
// that a lockfile change without regeneration fails the licensing gate.

import { spawnSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const CHECK = process.argv.includes("--check");
const OUT_LICENSES = resolve(ROOT, "LICENSES.md");
const OUT_NOTICES = resolve(ROOT, "apps", "desktop", "src-tauri", "vendor", "libmpv", "THIRD-PARTY-NOTICES.txt");
const LOCKFILE = resolve(ROOT, "pnpm-lock.yaml");
const DESKTOP_PACKAGE = resolve(ROOT, "apps", "desktop", "package.json");

// npm production license metadata, verified against node_modules below.
// A new production dependency must be added here explicitly, which is exactly
// the review step the licensing gate is meant to force.
const NPM_LICENSE_METADATA = {
  "@material/material-color-utilities": {
    license: "Apache-2.0",
    repository: "https://github.com/material-foundation/material-color-utilities",
  },
  "lucide-react": {
    license: "ISC",
    repository: "https://github.com/lucide-icons/lucide",
  },
  react: {
    license: "MIT",
    repository: "https://github.com/facebook/react",
  },
  "react-dom": {
    license: "MIT",
    repository: "https://github.com/facebook/react",
  },
};

// SPDX token -> section id in THIRD-PARTY-LICENSES.txt. Compound expressions
// are split into tokens; every resolved id must have an `== SPDX:` section.
const SECTION_ALIASES = {
  "GPL-2.0": "GPL-2.0",
  "GPL-2.0-only": "GPL-2.0",
  "GPL-2.0-or-later": "GPL-2.0",
  "LGPL-2.1": "LGPL-2.1",
  "LGPL-2.1-only": "LGPL-2.1",
  "LGPL-2.1-or-later": "LGPL-2.1",
  ISC: "ISC",
  MIT: "MIT",
  "MIT-0": "MIT-0",
  "Apache-2.0": "Apache-2.0",
  "LLVM-exception": "LLVM-exception",
  FTL: "FTL",
  Zlib: "Zlib",
  "BSD-2-Clause": "BSD-2-Clause",
  "BSD-3-Clause": "BSD-3-Clause",
  "MPL-2.0": "MPL-2.0",
  "Unicode-3.0": "Unicode-3.0",
  "CC0-1.0": "CC0-1.0",
  Unlicense: "Unlicense",
  "0BSD": "0BSD",
  "BSL-1.0": "BSL-1.0",
  libpng: "libpng-2.0",
  "libpng-2.0": "libpng-2.0",
};
const IGNORED_TOKENS = new Set(["", "OR", "AND", "WITH", "License"]);

function fail(message) {
  process.stderr.write(`generate-third-party-notices: ${message}\n`);
  process.exit(1);
}

function spdxSections(expression) {
  const ids = new Set();
  for (const raw of String(expression).replace(/;/g, " ").split(/[\s()\/,]+/)) {
    const token = raw.trim();
    if (IGNORED_TOKENS.has(token) || token.startsWith("LicenseRef-")) continue;
    ids.add(SECTION_ALIASES[token] ?? `UNMAPPED:${token}`);
  }
  return [...ids];
}

function collectRust() {
  const result = spawnSync("cargo", ["metadata", "--format-version", "1"], {
    cwd: ROOT,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  if (result.error) fail(`cannot run cargo metadata: ${result.error.message}`);
  if (result.status !== 0) {
    fail(`cargo metadata exited ${result.status}: ${(result.stderr || "").slice(0, 800)}`);
  }
  let metadata;
  try {
    metadata = JSON.parse(result.stdout);
  } catch {
    fail("cargo metadata did not return valid JSON");
  }
  const map = (p) => ({
    name: p.name,
    version: p.version,
    license: (p.license ?? "UNSPECIFIED").trim() || "UNSPECIFIED",
    source: (p.repository ?? p.homepage ?? `https://crates.io/crates/${p.name}`).trim(),
  });
  const thirdParty = metadata.packages.filter((p) => p.source !== null).map(map);
  // Workspace members inherit [workspace.package].repository; do not fall
  // back to a crates.io URL that does not exist (publish = false).
  const workspaceRepo =
    (readFileSync(resolve(ROOT, "Cargo.toml"), "utf8").match(/^repository\s*=\s*"([^"]+)"/m) ?? [])[1] ??
    "https://github.com/zsxink/Echo";
  const workspace = metadata.packages
    .filter((p) => p.source === null)
    .map((p) => ({ ...map(p), source: workspaceRepo }));
  thirdParty.sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version));
  workspace.sort((a, b) => a.name.localeCompare(b.name));
  return { thirdParty, workspace };
}

function collectNpm() {
  const pkg = JSON.parse(readFileSync(DESKTOP_PACKAGE, "utf8"));
  const dependencyNames = Object.keys(pkg.dependencies ?? {}).sort();
  const metadataNames = Object.keys(NPM_LICENSE_METADATA).sort();
  if (dependencyNames.join("\n") !== metadataNames.join("\n")) {
    fail(
      `production npm dependencies changed without updating NPM_LICENSE_METADATA\nexpected: ${dependencyNames.join(", ")}\nrecorded: ${metadataNames.join(", ")}`,
    );
  }

  // Resolve versions from the `apps/desktop` importer's `dependencies` block.
  // pnpm lockfiles are YAML; this parser only reads the two indentation levels
  // it needs and deliberately stops at `devDependencies`.
  const versions = {};
  const lines = readFileSync(LOCKFILE, "utf8").split("\n");
  let inImporter = false;
  let inDependencies = false;
  let current = null;
  for (const line of lines) {
    if (/^  apps\/desktop:\s*$/.test(line)) {
      inImporter = true;
      continue;
    }
    if (!inImporter) continue;
    if (/^  \S/.test(line)) break;
    if (/^    dependencies:\s*$/.test(line)) {
      inDependencies = true;
      continue;
    }
    if (inDependencies && /^    \S/.test(line)) break;
    if (!inDependencies) continue;
    const entry = line.match(/^      '?([^':]+)'?:\s*$/);
    if (entry) {
      current = entry[1];
      continue;
    }
    const version = line.match(/^        version:\s*(\S+)/);
    if (version && current) {
      versions[current] = version[1].split("(")[0];
      current = null;
    }
  }
  const packages = dependencyNames.map((name) => {
    const version = versions[name];
    if (!version) fail(`pnpm-lock.yaml has no resolved version for ${name}`);
    const meta = NPM_LICENSE_METADATA[name];
    const installed = resolve(ROOT, "apps", "desktop", "node_modules", name, "package.json");
    if (existsSync(installed)) {
      const disk = JSON.parse(readFileSync(installed, "utf8"));
      if (disk.license !== meta.license) {
        fail(`${name} installed license is ${disk.license}, recorded ${meta.license}`);
      }
      if (disk.version !== version) {
        fail(`${name} installed version ${disk.version} does not match lockfile ${version}`);
      }
    }
    return { name, version, license: meta.license, source: meta.repository };
  });
  return packages;
}

function renderLicensesMd(rust, npm, sectionIds, workspace) {
  const rustRows = rust
    .map((p) => `| ${p.name} | ${p.version} | ${p.license} | ${p.source} |`)
    .join("\n");
  const npmRows = npm
    .map((p) => `| ${p.name} | ${p.version} | ${p.license} | ${p.source} |`)
    .join("\n");
  const workspaceRows = workspace
    .map((p) => `| \`${p.name}\` | ${p.version} | ${p.license}* | ${p.source} |`)
    .join("\n");
  return `# Echo — Third-Party Notices & License Inventory

This file is the human-readable license inventory for Echo's dependency graph.
It is **generated** by \`scripts/release/generate-third-party-notices.mjs\`
from \`cargo metadata\` and the \`apps/desktop\` importer of \`pnpm-lock.yaml\`;
do not edit by hand. \`task-1.7\` checks that the inventory exists, and
\`task-18.1\` runs the generator with \`--check\` so a lockfile change without
regeneration fails the licensing gate. \`cargo deny check\` enforces the Rust
license policy continuously.

The Rust table is a **conservative superset**: it lists the complete lockfile
graph, including dev-dependencies and target-specific crates that may not end
up in a given installer. Over-listing is intentional — it cannot create a
compliance gap. Only the \`dependencies\` (production) npm packages are listed;
build-time dev dependencies are not part of the repackaged artifact's license
surface. License expressions are reproduced from each package's manifest
verbatim, including \`OR\` / \`AND\` / \`WITH\` combinations; the graph contains
both permissive licenses and weak-copyleft licenses such as MPL-2.0.

Echo itself is licensed under **Echo Personal Study License 1.0**, an
independent custom license (root [\`LICENSE\`](LICENSE)) — personal
study/research use only, no commercial use, no resale, no redistribution. It
borrows BUSL-1.1's Change Date / Change License structure but is **not**
Business Source License 1.1 and must not be labeled with that SPDX id; custom
licenses are expressed in SPDX via the \`LicenseRef-\` mechanism. Workspace
members (\`echo-app\`, \`echo-core\`, \`echo-desktop\`) report
\`LicenseRef-Echo-Personal-Study-1.0\` in their manifests via the workspace
\`license\` field.

> **libmpv note:** the platform player binary (libmpv + FFmpeg siblings,
> bundled separately at packaging time) is **not** a Cargo or npm dependency
> of this repository, so it is intentionally not listed in the tables below.
> Its per-platform licensing follows each platform's actual build
> configuration and is pinned per platform by the vendor manifests
> (\`apps/desktop/src-tauri/vendor/libmpv/{macos,windows,linux}/manifest.json\`):
> **macOS** is the Echo self-build with FFmpeg's GPL and nonfree options
> disabled (LGPL-2.1-or-later; auxiliary libraries ISC/MIT/LGPL per the macOS
> NOTICE); **Windows** uses the shinchiro \`mpv-winbuild-cmake\` build whose
> FFmpeg keeps \`--enable-gpl\` and links x264/x265, so the bundled
> \`libmpv-2.dll\` is **GPL-2.0-or-later** (plus the Apache-2.0 Vulkan
> loader); **Linux** is the CI self-build whose FFmpeg keeps mpv-build's
> \`--enable-gpl\`, also **GPL-2.0-or-later**. The complete license texts ship
> inside every installer (\`THIRD-PARTY-LICENSES.txt\`), and the full
> dependency inventory ships alongside as \`THIRD-PARTY-NOTICES.txt\`.
> See each platform's \`NOTICE.md\` for corresponding-source pointers.

## Workspace members

| Crate | Version | License | Source |
| --- | --- | --- | --- |
${workspaceRows}

\\* Echo Personal Study License 1.0 — an independent custom license, see root \`LICENSE\`.

## Rust dependencies (from \`cargo metadata\`)

Complete lockfile graph, ${rust.length} third-party packages (conservative superset, see above).

| Crate | Version | License | Source |
| --- | --- | --- | --- |
${rustRows}

## Frontend production dependencies (from \`pnpm-lock.yaml\`, \`apps/desktop\`)

${npm.length} packages; dev dependencies are not part of the shipped artifact.

| Package | Version | License | Source |
| --- | --- | --- | --- |
${npmRows}

## SPDX sections referenced by this inventory

${sectionIds.map((id) => `- \`${id}\``).join("\n")}

Each id has an \`== SPDX: <id>\` section in
\`apps/desktop/src-tauri/vendor/libmpv/THIRD-PARTY-LICENSES.txt\`, which is
mapped into all three installers together with \`THIRD-PARTY-NOTICES.txt\`.
Individual component copyright notices remain in each upstream source package;
the merged text supplies the official license bodies. Verifying that pairing
against a specific release is part of the pre-release legal review.

## Regeneration

\`\`\`sh
node scripts/release/generate-third-party-notices.mjs
node scripts/release/generate-third-party-notices.mjs --check
\`\`\`

Run regeneration after any Cargo.lock / pnpm-lock.yaml / production
dependency change, and after adding a new license family to
\`THIRD-PARTY-LICENSES.txt\`.
`;
}

function renderNotices(rust, npm, sectionIds) {
  const rustRows = rust
    .map((p) => `  ${p.name} ${p.version} | ${p.license} | ${p.source}`)
    .join("\n");
  const npmRows = npm
    .map((p) => `  ${p.name} ${p.version} | ${p.license} | ${p.source}`)
    .join("\n");
  return `Echo — Third-party dependency notices / 第三方依赖告知
===============================================================================

This file ships inside every Echo desktop installer (macOS
Contents/Resources/third-party/libmpv/, Windows install root, Linux
/usr/share/doc/echo/). It lists every third-party component in Echo's
dependency graph and points at the official license texts in the companion
file THIRD-PARTY-LICENSES.txt, which ships in the same directory.

本文件随 Echo 三个桌面平台的安装包分发，列出依赖图中的全部第三方组件，
并指向同目录 THIRD-PARTY-LICENSES.txt 中对应许可的官方全文。

SCOPE / 范围
  The Rust list is a conservative superset generated from the complete
  Cargo.lock graph via \`cargo metadata\`, including development and
  target-specific crates that may not be present in a given installer.
  Only production npm dependencies are listed. Over-listing is intentional
  and does not grant or remove any license rights.
  Rust 清单为完整锁文件图的保守超集；npm 仅列生产依赖。

COPYRIGHT NOTICES / 版权声明
  Each component's copyright notice remains in its upstream source package.
  THIRD-PARTY-LICENSES.txt supplies the official license bodies for every
  SPDX id listed below. Echo's own source is licensed separately under the
  Echo Personal Study License 1.0 (LICENSE); it does not restrict any right
  granted by a third-party component's upstream license.
  各组件的版权声明以其上游源码包为准；本文件不替代、也不减损上游许可。

SPDX identifiers referenced by this inventory:
${sectionIds.map((id) => `  ${id}`).join("\n")}

-------------------------------------------------------------------------------
Rust dependencies (${rust.length} packages, conservative superset)
-------------------------------------------------------------------------------
${rustRows}

-------------------------------------------------------------------------------
Frontend production dependencies (${npm.length} packages)
-------------------------------------------------------------------------------
${npmRows}

-------------------------------------------------------------------------------
License texts / 许可全文
-------------------------------------------------------------------------------
  See THIRD-PARTY-LICENSES.txt, section "== SPDX: <id>" for each id above.
  Platform playback binaries and their corresponding-source pointers are
  documented in the per-platform NOTICE.md files under vendor/libmpv/.
  各平台播放库二进制的许可与对应源码指引见 vendor/libmpv/*/NOTICE.md。
`;
}

const { thirdParty: rust, workspace } = collectRust();
const npm = collectNpm();
const sectionIds = [
  ...new Set([...rust, ...npm].flatMap((p) => spdxSections(p.license))),
].sort();
const unmapped = sectionIds.filter((id) => id.startsWith("UNMAPPED:"));
if (unmapped.length > 0) {
  fail(`dependency graph references SPDX ids without a section mapping: ${unmapped.join(", ")}`);
}

const outputs = [
  [OUT_LICENSES, renderLicensesMd(rust, npm, sectionIds, workspace)],
  [OUT_NOTICES, renderNotices(rust, npm, sectionIds)],
];

if (CHECK) {
  let stale = false;
  for (const [path, expected] of outputs) {
    if (!existsSync(path)) {
      process.stderr.write(`MISSING: ${path.replace(ROOT + "/", "")}\n`);
      stale = true;
      continue;
    }
    const actual = readFileSync(path, "utf8");
    if (actual !== expected) {
      process.stderr.write(`STALE: ${path.replace(ROOT + "/", "")} (regenerate with the script)\n`);
      const expectedLines = expected.split("\n");
      const firstDiff = expectedLines.findIndex((line, i) => line !== actual.split("\n")[i]);
      if (firstDiff !== -1) {
        process.stderr.write(`  first difference at line ${firstDiff + 1}: ${expectedLines[firstDiff].slice(0, 120)}\n`);
      }
      stale = true;
    }
  }
  if (stale) process.exit(1);
  process.stdout.write(
    `ok third-party inventory: LICENSES.md + THIRD-PARTY-NOTICES.txt match lockfiles (${rust.length} Rust + ${npm.length} npm, ${sectionIds.length} SPDX sections)\n`,
  );
} else {
  for (const [path, content] of outputs) writeFileSync(path, content);
  process.stdout.write(
    `wrote LICENSES.md and THIRD-PARTY-NOTICES.txt (${rust.length} Rust + ${npm.length} npm, ${sectionIds.length} SPDX sections)\n`,
  );
}
