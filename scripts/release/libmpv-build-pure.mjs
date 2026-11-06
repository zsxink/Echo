#!/usr/bin/env node
// Pure helpers for the pinned macOS libmpv/FFmpeg build: argument parsing,
// guard predicates, digest / option / otool / NOTICE parsing and the small
// provenance writers. Nothing here touches the filesystem or spawns a process,
// which is why the cross-platform self-test can import every export. The
// destructive path guard (assertSafeScratchDir) lives here too so it is
// exercised before any rmSync in the orchestration module.

import { homedir } from "node:os";
import { basename, resolve, sep } from "node:path";
import {
  BUILT_DYLIBS,
  MACOSX_DEPLOYMENT_TARGET,
  NOTICE_BEGIN,
  NOTICE_END,
  ROOT,
  SYSTEM_PREFIXES,
  VENDOR,
} from "./libmpv-build-config.mjs";

export function fail(message) {
  throw new Error(message);
}

function requireValue(argv, index, flag) {
  const value = argv[index];
  if (!value || value.startsWith("--")) throw new Error(`${flag} requires a value`);
  return resolve(value);
}

export function parseArgs(argv) {
  const opts = { work: undefined, out: undefined, writeVendor: false, help: false };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--work") {
      opts.work = requireValue(argv, ++i, "--work");
    } else if (arg === "--out") {
      opts.out = requireValue(argv, ++i, "--out");
    } else if (arg === "--write-vendor") {
      opts.writeVendor = true;
    } else if (arg === "--help" || arg === "-h") {
      opts.help = true;
    } else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }
  if (!opts.help) {
    if (!opts.out) throw new Error("missing --out <dir>");
    if (!opts.writeVendor && !opts.work) throw new Error("missing --work <dir>");
  }
  return opts;
}

export function assertHost(platform, arch) {
  if (platform !== "darwin" || arch !== "arm64") {
    throw new Error(
      `this build must run on darwin/arm64, got ${platform}/${arch}; cross-arch output is not a release artifact`,
    );
  }
}

export function assertSafeScratchDir(dir, root = ROOT) {
  // The builder recursively deletes --work and --out before writing. Refuse
  // paths whose deletion could remove the repository, the vendor tree, the
  // user's home, or a filesystem root; a typo in an argument must fail before
  // rmSync runs, not after.
  const target = resolve(dir);
  const repository = resolve(root);
  if (target === resolve(sep)) fail(`refusing to delete filesystem root: ${target}`);
  if (target.split(sep).filter(Boolean).length < 2) fail(`refusing to use a top-level directory as scratch space: ${target}`);
  if (target === repository || repository.startsWith(`${target}${sep}`)) {
    fail(`refusing to delete ${target}: it contains the repository ${repository}`);
  }
  if (target.startsWith(`${repository}${sep}`)) {
    fail(`refusing to use ${target} inside the repository as scratch space`);
  }
  if (target === VENDOR || target.startsWith(`${VENDOR}${sep}`)) {
    fail(`refusing to use the vendor directory as scratch space: ${target}`);
  }
  // The home directory and any of its first-level children (Desktop,
  // Documents, Downloads, ...) hold the user's real data. Wiping one of those
  // would be just as destructive as wiping home itself, so a scratch path must
  // be at least two levels beneath home. Nested build dirs (e.g.
  // ~/work/echo-build) remain allowed; the important guarantee is that
  // `~/Desktop` can never be handed to rmSync.
  const home = resolve(homedir());
  if (target === home) {
    fail(`refusing to delete the user's home directory: ${target}`);
  }
  if (target.startsWith(`${home}${sep}`)) {
    const belowHome = target.slice(home.length + sep.length);
    if (!belowHome.includes(sep)) {
      fail(
        `refusing to use ${target}: a first-level child of the user's home cannot be used as scratch space`,
      );
    }
  }
  return target;
}

export function verifyPinnedDigest({ name, expected, actual }) {
  if (!/^[0-9a-f]{64}$/.test(String(expected ?? ""))) {
    throw new Error(`${name} has no 64-hex SHA-256 pin`);
  }
  if (actual !== expected) {
    throw new Error(
      `${name} SHA-256 mismatch: expected ${expected}, got ${actual}`,
    );
  }
}

export function parseMesonOptionNames(source) {
  // meson_options.txt declares one option per `option('name', type: ...)`
  // call. Only the name matters here: values are validated by meson itself.
  const names = new Set();
  const pattern = /^\s*option\(\s*'([^']+)'/gm;
  for (const match of String(source ?? "").matchAll(pattern)) {
    names.add(match[1]);
  }
  if (names.size === 0) throw new Error("meson_options.txt declared no options");
  return names;
}

export function optionName(option) {
  const match = /^-D([^=]+)=/.exec(String(option));
  if (!match) throw new Error(`not a -D option: ${option}`);
  return match[1];
}

export function findUnknownMesonOptions(options, declaredNames) {
  const declared = declaredNames instanceof Set ? declaredNames : new Set(declaredNames);
  return options.map(optionName).filter((name) => !declared.has(name)).sort();
}

export function findConfigureWarnings(output) {
  // configure only warns when an --enable-* component matches nothing. A typo
  // in the component list would otherwise ship a smaller-than-declared runtime.
  return String(output ?? "")
    .split("\n")
    .filter((line) => /did not match anything/i.test(line))
    .map((line) => line.trim());
}

export function rewritePkgConfigPrefix(text, stagingPrefix) {
  // FFmpeg installs .pc files with the configure-time prefix (/usr/local).
  // Pointing them at the DESTDIR staging tree is what makes mpv link the
  // just-built FFmpeg 6.0 instead of any Homebrew FFmpeg on the host.
  // FFmpeg 6.0 writes absolute libdir=/usr/local/lib and
  // includedir=/usr/local/include lines, so rewriting prefix= alone would
  // still emit -I/usr/local/include; every absolute path under the original
  // prefix must move together. Values expressed as ${prefix}/... already
  // follow the rewritten prefix and are left untouched.
  const lines = String(text ?? "").split("\n");
  const originalPrefix = lines
    .map((line) => /^prefix=(\/.*)$/.exec(line)?.[1])
    .find(Boolean) ?? "/usr/local";
  return lines
    .map((line) => {
      const match = /^(prefix|exec_prefix|libdir|includedir)=(\/.*)$/.exec(line);
      if (!match) return line;
      const [, key, value] = match;
      const relative = value === originalPrefix ? "" : value.startsWith(`${originalPrefix}/`)
        ? value.slice(originalPrefix.length)
        : null;
      return relative === null ? line : `${key}=${stagingPrefix}${relative}`;
    })
    .join("\n");
}

export function parseOtoolLibraries(output) {
  // `otool -L` prints the inspected path first, then one reference per line.
  // The first reference is the file's own LC_ID_DYLIB, not a dependency; the
  // install-name planner drops self references.
  return String(output ?? "")
    .split("\n")
    .slice(1)
    .map((line) => line.trim().split(/\s+/)[0])
    .filter((ref) => ref.startsWith("/") || ref.startsWith("@"));
}

export function normalizeDylibName(name) {
  // Release convention: FFmpeg and mpv are vendored under their unversioned
  // names, auxiliary Homebrew libraries keep theirs (libass.9.dylib).
  const ffmpeg = /^lib(avcodec|avformat|avutil|avfilter|swresample|swscale)(?:\.\d+)+\.dylib$/.exec(name);
  if (ffmpeg) return `lib${ffmpeg[1]}.dylib`;
  const mpv = /^libmpv(?:\.\d+)+\.dylib$/.exec(name);
  if (mpv) return "libmpv.dylib";
  return name;
}

// Parse a single `otool -D <path>` output. The first line is the inspected
// path; the remaining non-empty lines are LC_ID_DYLIB names. Kept here so the
// post-rewrite verification in copyAndRewrite shares the exact same view of
// the install name as the planner that produced it.
export function parseOtoolId(output) {
  return String(output ?? "")
    .split("\n")
    .slice(1)
    .map((line) => line.trim())
    .filter(Boolean);
}

export function classifyDependencyPath(reference, { homebrewPrefix, stagingPrefixes }) {
  const ref = String(reference ?? "");
  const name = ref.slice(ref.lastIndexOf("/") + 1);
  if (ref.startsWith("@rpath/") || ref.startsWith("@loader_path/") || ref.startsWith("@executable_path/")) {
    return { kind: "local", name, target: normalizeDylibName(name) };
  }
  if (ref.startsWith("/")) {
    if ((stagingPrefixes ?? []).some((prefix) => ref.startsWith(`${prefix}/`))) {
      return { kind: "staging", name, target: normalizeDylibName(name) };
    }
    if (homebrewPrefix && ref.startsWith(`${homebrewPrefix}/`)) {
      return { kind: "homebrew", name, target: normalizeDylibName(name) };
    }
    if (SYSTEM_PREFIXES.some((prefix) => ref.startsWith(`${prefix}/`))) {
      return { kind: "system", name, target: null };
    }
  }
  return { kind: "unknown", name, target: null };
}

export function planInstallNameChanges({ files, referencesByFile }) {
  // Turn raw otool references into install_name_tool operations. Files in the
  // candidate set get @loader_path siblings; system references stay absolute.
  // Unknown references are reported, never silently dropped.
  const fileSet = new Set(files);
  const ids = [];
  const changes = [];
  const systemReferences = [];
  const unknownReferences = [];
  for (const file of files) {
    ids.push({ file, installName: `@rpath/${file}` });
    for (const reference of referencesByFile[file] ?? []) {
      const refName = reference.slice(reference.lastIndexOf("/") + 1);
      if (reference.startsWith("/")) {
        if (SYSTEM_PREFIXES.some((prefix) => reference.startsWith(`${prefix}/`))) {
          systemReferences.push({ file, reference });
        } else {
          const target = normalizeDylibName(refName);
          if (fileSet.has(target)) {
            const to = `@loader_path/${target}`;
            if (reference !== to) changes.push({ file, from: reference, to });
          } else {
            unknownReferences.push({ file, reference });
          }
        }
        continue;
      }
      const target = normalizeDylibName(refName);
      if (!fileSet.has(target)) {
        unknownReferences.push({ file, reference });
        continue;
      }
      if (target === file) continue; // LC_ID_DYLIB self reference
      const to = `@loader_path/${target}`;
      if (reference !== to) changes.push({ file, from: reference, to });
    }
  }
  return { ids, changes, systemReferences, unknownReferences };
}

export function renderNoticeBlock(auxiliary) {
  const rows = [...auxiliary]
    .sort((a, b) => a.component.localeCompare(b.component))
    .map((entry) => `| ${entry.component} | Homebrew ${entry.version} |`);
  return [
    NOTICE_BEGIN,
    "| Component | Version / provenance |",
    "| --- | --- |",
    ...rows,
    NOTICE_END,
  ].join("\n");
}

// Count non-overlapping occurrences of a literal sentinel. Used to reject a
// NOTICE file whose generated block is malformed before we touch it: a missing
// END, a second BEGIN, or an END that precedes its BEGIN would otherwise leave
// stale rows behind (the old regex replaced only one complete BEGIN..END pair
// and silently appended a second block when END was absent).
function countOccurrences(text, needle) {
  if (needle === "") return 0;
  let count = 0;
  let from = 0;
  for (;;) {
    const index = text.indexOf(needle, from);
    if (index === -1) return count;
    count += 1;
    from = index + needle.length;
  }
}

export function mergeNoticeBlock(notice, block) {
  // The generated block is the only machine-owned region; legal prose outside
  // it stays untouched. Replacing an existing block makes the writer idempotent.
  const text = String(notice ?? "");
  const beginCount = countOccurrences(text, NOTICE_BEGIN);
  const endCount = countOccurrences(text, NOTICE_END);
  const hasSentinels = beginCount > 0 || endCount > 0;
  if (hasSentinels) {
    const beginIndex = text.indexOf(NOTICE_BEGIN);
    const endIndex = text.indexOf(NOTICE_END);
    // A well-formed file has exactly one BEGIN and one END, with BEGIN first.
    // Anything else is a corrupted/partial generated block; refusing is safer
    // than producing two BEGIN blocks and a stale version table.
    if (beginCount !== 1 || endCount !== 1 || beginIndex === -1 || endIndex === -1 || endIndex < beginIndex) {
      fail(
        `NOTICE generated block is malformed: found ${beginCount} BEGIN and ${endCount} END sentinels; ` +
          "expected exactly one BEGIN followed by one END",
      );
    }
    const pattern = new RegExp(
      `${escapeRegExp(NOTICE_BEGIN)}[\\s\\S]*?${escapeRegExp(NOTICE_END)}`,
    );
    return `${text.replace(pattern, block).replace(/\s*$/, "")}\n`;
  }
  return `${text.replace(/\s*$/, "")}\n\n${block}\n`;
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

// `patch --forward` reports a rejected/already-applied patch through the spawn
// result. Kept pure so the self-test can inject a synthetic result without
// running the macOS `patch` binary.
export function assertPatchResult(result, patchName) {
  if (result.error) fail(`${patchName} failed to start: ${result.error.message}`);
  if (result.status !== 0) {
    const detail = `${result.stdout || ""}${result.stderr || ""}`.trim();
    fail(`${patchName} was rejected by the pinned mpv source${detail ? `\n${detail}` : ""}`);
  }
}

function enabledComponents(configure, flag) {
  const prefix = `--enable-${flag}=`;
  const value = (configure ?? []).find((argument) => String(argument).startsWith(prefix));
  if (!value) return [];
  return String(value).slice(prefix.length).split(",").filter(Boolean);
}

export function summarizeFfmpegBuild(builds) {
  const filters = enabledComponents(builds?.ffmpeg?.configure, "filter");
  return (
    `FFmpeg ${builds?.ffmpeg?.version ?? "unknown"}; LGPL-only; ` +
    `deployment target ${builds?.ffmpeg?.deploymentTarget ?? "unknown"}; ` +
    `required filters ${filters.length > 0 ? filters.join(",") : "none"}`
  );
}

export function summarizeMpvBuild(builds) {
  const patches = builds?.mpv?.patches ?? [];
  const hardDependencies = builds?.mpv?.hardDependencies ?? [];
  return (
    `mpv ${builds?.mpv?.version ?? "unknown"}; ` +
    `${patches.length} patch(es) applied${patches.length > 0 ? `: ${patches.map((path) => basename(path)).join(", ")}` : ""}; ` +
    `hard dependencies ${hardDependencies.length > 0 ? hardDependencies.join(", ") : "none"}`
  );
}

export function applyBuildRecords(manifest, { files, fileHashes, builds, buildHost }) {
  // Top-level sourceUrl / assetSha256 / additionalSources remain the sole pin
  // authority; this only adds the machine-readable record of the build itself.
  const next = JSON.parse(JSON.stringify(manifest));
  next.buildScript = "scripts/release/build-macos-libmpv.mjs";
  next.files = Object.fromEntries([...files].sort().map((file) => [file, fileHashes[file]]));
  next.fileArchitectures = Object.fromEntries(
    [...files].sort().map((file) => [file, [...manifest.architectures].sort()]),
  );
  next.builds = builds;
  next.buildHost = buildHost;
  next.ffmpegBuild = summarizeFfmpegBuild(builds);
  next.mpvBuild = summarizeMpvBuild(builds);
  next.buildProcedureStatus =
    "Reproduced from the pinned sources by scripts/release/build-macos-libmpv.mjs; " +
    "every bundled dylib is arm64-only and the runtime component probe passed before publishing.";
  return next;
}

// Parse an `otool -l` load-command dump down to the minos field of every
// LC_BUILD_VERSION command. The version is anchored to a whole line inside a
// build-version block (formatted `    minos 27.0`), so path text, LC_VERSION_MIN
// or another command's `version` field cannot be mistaken for it. A file with
// several build-version blocks must agree on a single minos; returning the
// first block silently would hide a drift. Only `数字(.数字)*` with no trailing
// dot is accepted.
export function parseMinos(otoolOutput) {
  // Walk the load commands line by line and stay "inside" a command until the
  // next `cmd ...` line. A `minos` field only counts while the current command
  // is LC_BUILD_VERSION, so path text, LC_VERSION_MIN_MACOSX's `version`, and
  // another command's fields can never be mistaken for it. This also works for
  // synthetic dumps that start directly at `cmd LC_BUILD_VERSION` (no
  // `Load command N` header), which the self-test relies on.
  const values = [];
  let inBuildVersion = false;
  for (const line of String(otoolOutput ?? "").split("\n")) {
    if (/^\s*Load command \d+\s*$/.test(line)) {
      inBuildVersion = false;
      continue;
    }
    if (/^\s*cmd\s+\S+/.test(line)) {
      // `cmdsize ...` does not match: there is no whitespace after `cmd`.
      inBuildVersion = /^\s*cmd\s+LC_BUILD_VERSION\b/.test(line);
      continue;
    }
    if (!inBuildVersion) continue;
    const match = /^[ \t]*minos[ \t]+([0-9]+(?:\.[0-9]+)*)[ \t]*$/.exec(line);
    if (match) values.push(match[1]);
  }
  if (values.length === 0) return null;
  const distinct = [...new Set(values)];
  if (distinct.length !== 1) {
    fail(
      `otool -l reports conflicting LC_BUILD_VERSION minos values: ${distinct.join(", ")}`,
    );
  }
  return distinct[0];
}

export function compareDottedVersions(left, right) {
  const a = String(left).split(".").map(Number);
  const b = String(right).split(".").map(Number);
  const length = Math.max(a.length, b.length);
  for (let index = 0; index < length; index += 1) {
    const difference = (a[index] ?? 0) - (b[index] ?? 0);
    if (difference !== 0) return difference < 0 ? -1 : 1;
  }
  return 0;
}

export function deploymentTargetViolation(file, minos, expected = MACOSX_DEPLOYMENT_TARGET) {
  if (!minos) return `${file} has no LC_BUILD_VERSION minos`;
  const comparison = compareDottedVersions(minos, expected);
  if (comparison > 0) {
    return `${file} targets macOS ${minos}, newer than the pinned deployment target ${expected}`;
  }
  if (BUILT_DYLIBS.includes(file) && comparison !== 0) {
    return `${file} targets macOS ${minos}, expected deployment target ${expected}`;
  }
  return null;
}

export function homebrewVersionFromSource(sourcePath, formula) {
  // The version must come from the exact file that entered the closure. A
  // separate `brew list --versions` can report another keg when multiple
  // versions are installed, which would record a version that is not what was
  // copied. Homebrew lays every keg out as <prefix>/Cellar/<formula>/<version>/.
  const pattern = new RegExp(`[\\\\/]Cellar[\\\\/]${escapeRegExp(formula)}[\\\\/]([^\\\\/]+)[\\\\/]`);
  const match = pattern.exec(String(sourcePath ?? ""));
  if (!match) {
    fail(
      `cannot read ${formula} version from closure source ${sourcePath}; ` +
        "expected a Homebrew Cellar path",
    );
  }
  return match[1];
}
