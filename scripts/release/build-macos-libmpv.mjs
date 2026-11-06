#!/usr/bin/env node
// Build Echo's pinned macOS libmpv/FFmpeg audio stack from source.
//
// The vendor manifest is the only source of pins: `sourceUrl` / `assetSha256`
// for FFmpeg, `additionalSources` for mpv and its patches. A build either
// reproduces those inputs exactly or fails; nothing is resolved from the
// network implicitly.
//
//   node scripts/release/build-macos-libmpv.mjs --work <dir> --out <dir>
//   node scripts/release/build-macos-libmpv.mjs --out <dir> --write-vendor
//
// The first form builds a candidate in <out> and probes it; the vendor tree is
// untouched. The second form copies an already-probed candidate into the repo
// vendor, records provenance in manifest.json / NOTICE.md, and then runs
// thin-macos-libmpv.mjs --write for strip, ad-hoc re-sign and SHA-256 re-pins.
// Splitting the two keeps a failed build from dirtying the release tree.
//
// This is a maintainer tool. Only the pure helpers are exercised by the
// cross-platform self-test (scripts/release/build-macos-libmpv.test.mjs).
//
// The implementation is split by responsibility so no single file exceeds the
// 1000-line source limit:
//   - libmpv-build-config.mjs        constants, pins and repository paths
//   - libmpv-build-pure.mjs          pure guards / parsers / provenance writers
//   - libmpv-build-orchestration.mjs downloads, builds, otool, vendor writer
// This module is the stable public entry: it re-exports every symbol the self-
// test and the Gates (task-1.10.mjs, ffmpeg-component-probe.test.mjs) import,
// and it is the only module with a CLI guard. Importing it has no side effects.

import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "./libmpv-build-orchestration.mjs";

export {
  AUXILIARY_COMPONENTS,
  BUILT_DYLIBS,
  FFMPEG_CONFIGURE_ARGS,
  MACOSX_DEPLOYMENT_TARGET,
  MPV_HARD_DEPENDENCIES,
  MPV_MESON_OPTIONS,
} from "./libmpv-build-config.mjs";

export {
  applyBuildRecords,
  assertHost,
  assertPatchResult,
  assertSafeScratchDir,
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
  rewritePkgConfigPrefix,
  summarizeFfmpegBuild,
  summarizeMpvBuild,
  verifyPinnedDigest,
} from "./libmpv-build-pure.mjs";

export {
  backupVendorTree,
  resolvePatchPaths,
  restoreVendorTree,
} from "./libmpv-build-orchestration.mjs";

const invoked = process.argv[1] ? resolve(process.argv[1]) : "";
if (invoked === fileURLToPath(import.meta.url)) {
  try {
    build(process.argv.slice(2));
  } catch (error) {
    process.stderr.write(`FAIL build-macos-libmpv: ${error.message}\n`);
    process.exit(1);
  }
}
